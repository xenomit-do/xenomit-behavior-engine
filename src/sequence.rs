//! Stateful sequences for multi-hour tasks. One operation is issued once and
//! awaited across ticks; retries preserve its idempotency key.
use crate::{
    BehaviorModule, ModuleOutput, OperationHandle, OperationPoll, OperationRequest, TickContext,
};
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

#[derive(Clone, Copy, Debug)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff: Duration,
}
impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 1,
            backoff: Duration::ZERO,
        }
    }
}
type Condition = dyn FnMut(&TickContext<'_>, &BTreeMap<String, Value>) -> bool + Send;
type RequestFactory =
    dyn FnOnce(&BTreeMap<String, Value>) -> Result<OperationRequest, String> + Send;
enum Step {
    Operation {
        name: String,
        request: Option<OperationRequest>,
        factory: Option<Box<RequestFactory>>,
        timeout_ms: u64,
        retry: RetryPolicy,
    },
    Delay {
        name: String,
        duration_ms: u64,
    },
    Until {
        name: String,
        condition: Box<Condition>,
        timeout_ms: u64,
    },
}
#[derive(Clone, Debug, PartialEq)]
pub struct SequenceProgress {
    pub completed_steps: usize,
    pub total_steps: usize,
    pub current_step: Option<String>,
    pub attempt: u32,
}
pub struct TaskSequence {
    steps: Vec<Step>,
    index: usize,
    entered_at: Option<u64>,
    in_flight: Option<OperationHandle>,
    attempt: u32,
    retry_at: u64,
    results: BTreeMap<String, Value>,
    run_key: String,
    terminal: Option<ModuleOutput>,
}
impl Default for TaskSequence {
    fn default() -> Self {
        Self::new()
    }
}
impl TaskSequence {
    pub fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let epoch = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        Self {
            steps: vec![],
            index: 0,
            entered_at: None,
            in_flight: None,
            attempt: 0,
            retry_at: 0,
            results: BTreeMap::new(),
            run_key: format!("sequence-{epoch}-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
            terminal: None,
        }
    }
    pub fn operation(
        mut self,
        name: impl Into<String>,
        request: OperationRequest,
        timeout: Duration,
        retry: RetryPolicy,
    ) -> Result<Self, String> {
        if retry.max_attempts == 0 || timeout.is_zero() {
            return Err("operation timeout and attempts must be positive".into());
        }
        if retry.max_attempts > 1 && !request.idempotent {
            return Err("automatic retries require an idempotent service operation".into());
        }
        let name = name.into();
        self.validate_operation_name(&name)?;
        self.steps.push(Step::Operation {
            name,
            request: Some(request),
            factory: None,
            timeout_ms: milliseconds(timeout).max(1),
            retry,
        });
        Ok(self)
    }
    /// Build the next request from earlier results, once when the step is reached.
    /// Retries reuse the exact request and key, rather than repeating the factory.
    pub fn operation_with<F>(
        mut self,
        name: impl Into<String>,
        timeout: Duration,
        retry: RetryPolicy,
        factory: F,
    ) -> Result<Self, String>
    where
        F: FnOnce(&BTreeMap<String, Value>) -> Result<OperationRequest, String> + Send + 'static,
    {
        if retry.max_attempts == 0 || timeout.is_zero() {
            return Err("operation timeout and attempts must be positive".into());
        }
        let name = name.into();
        self.validate_operation_name(&name)?;
        self.steps.push(Step::Operation {
            name,
            request: None,
            factory: Some(Box::new(factory)),
            timeout_ms: milliseconds(timeout).max(1),
            retry,
        });
        Ok(self)
    }
    fn validate_operation_name(&self, name: &str) -> Result<(), String> {
        if name.is_empty()
            || self
                .steps
                .iter()
                .any(|s| matches!(s, Step::Operation { name: existing, .. } if existing == name))
        {
            return Err("operation names must be non-empty and unique".into());
        }
        Ok(())
    }
    pub fn delay(mut self, name: impl Into<String>, duration: Duration) -> Self {
        self.steps.push(Step::Delay {
            name: name.into(),
            duration_ms: milliseconds(duration),
        });
        self
    }
    pub fn wait_until<F>(mut self, name: impl Into<String>, timeout: Duration, condition: F) -> Self
    where
        F: FnMut(&TickContext<'_>, &BTreeMap<String, Value>) -> bool + Send + 'static,
    {
        self.steps.push(Step::Until {
            name: name.into(),
            condition: Box::new(condition),
            timeout_ms: milliseconds(timeout),
        });
        self
    }
    pub fn results(&self) -> &BTreeMap<String, Value> {
        &self.results
    }
    pub fn progress(&self) -> SequenceProgress {
        SequenceProgress {
            completed_steps: self.index,
            total_steps: self.steps.len(),
            current_step: self.steps.get(self.index).map(|step| match step {
                Step::Operation { name, .. }
                | Step::Delay { name, .. }
                | Step::Until { name, .. } => name.clone(),
            }),
            attempt: self.attempt,
        }
    }
    pub fn tick(&mut self, context: &mut TickContext<'_>) -> ModuleOutput {
        if let Some(output) = &self.terminal {
            return output.clone();
        }
        let output = self.tick_inner(context);
        if output.state != crate::RunState::Running {
            self.terminal = Some(output.clone());
        }
        output
    }
    fn tick_inner(&mut self, context: &mut TickContext<'_>) -> ModuleOutput {
        if context.cancellation.is_cancelled() {
            if let Some(handle) = self.in_flight.take() {
                context.operations.cancel(handle);
                context.operations.release(handle);
            }
            return ModuleOutput {
                state: crate::RunState::Cancelled,
                status: "Cancelled".into(),
                plan: None,
            };
        }
        let Some(step) = self.steps.get_mut(self.index) else {
            return ModuleOutput::completed("Sequence completed");
        };
        let entered = *self.entered_at.get_or_insert(context.now_ms);
        let name = match step {
            Step::Operation { name, .. } | Step::Delay { name, .. } | Step::Until { name, .. } => {
                name.clone()
            }
        };
        let mut complete = false;
        let mut failure = None;
        match step {
            Step::Delay { duration_ms, .. } => {
                complete = context.now_ms.saturating_sub(entered) >= *duration_ms
            }
            Step::Until {
                condition,
                timeout_ms,
                ..
            } => {
                complete = condition(context, &self.results);
                if !complete && context.now_ms.saturating_sub(entered) >= *timeout_ms {
                    return ModuleOutput::failed(format!("{name}: condition timed out"));
                }
            }
            Step::Operation {
                request,
                factory,
                timeout_ms,
                retry,
                ..
            } => {
                if request.is_none() {
                    match factory.take().expect("unbuilt operation has a factory")(&self.results) {
                        Ok(built) if retry.max_attempts > 1 && !built.idempotent => {
                            return ModuleOutput::failed(format!(
                                "{name}: automatic retries require an idempotent service operation"
                            ));
                        }
                        Ok(built) => *request = Some(built),
                        Err(error) => return ModuleOutput::failed(format!("{name}: {error}")),
                    }
                }
                let request = request.as_ref().expect("operation request built");
                if let Some(handle) = self.in_flight {
                    match context.operations.poll(handle) {
                        OperationPoll::Succeeded(value) => {
                            self.results.insert(name.clone(), value);
                            complete = true;
                        }
                        OperationPoll::Failed(error) => failure = Some(error),
                        OperationPoll::Pending
                            if context.now_ms.saturating_sub(entered) >= *timeout_ms =>
                        {
                            context.operations.cancel(handle);
                            failure = Some("operation timed out".into());
                        }
                        OperationPoll::Pending => {}
                    }
                    if complete || failure.is_some() {
                        context.operations.release(handle);
                        self.in_flight = None;
                    }
                } else if context.now_ms >= self.retry_at
                    && (!request.requires_game || context.game_ready)
                {
                    let mut request = request.clone();
                    if request.idempotency_key.is_none() {
                        request.idempotency_key =
                            Some(format!("{}-step-{}", self.run_key, self.index));
                    }
                    self.attempt += 1;
                    self.entered_at = Some(context.now_ms);
                    match context.operations.start(request) {
                        Ok(handle) => self.in_flight = Some(handle),
                        Err(error) => failure = Some(error),
                    }
                }
                if let Some(error) = failure {
                    if self.attempt >= retry.max_attempts {
                        return ModuleOutput::failed(format!("{name}: {error}"));
                    }
                    self.retry_at = context.now_ms.saturating_add(milliseconds(retry.backoff));
                    self.entered_at = None;
                    return ModuleOutput::running(format!(
                        "{name}: retry {} of {} ({error})",
                        self.attempt + 1,
                        retry.max_attempts
                    ));
                }
            }
        }
        if complete {
            self.index += 1;
            self.entered_at = None;
            self.attempt = 0;
            self.retry_at = 0;
        }
        if self.index == self.steps.len() {
            ModuleOutput::completed("Sequence completed")
        } else {
            ModuleOutput::running(format!(
                "{} / {}: {name}",
                self.index + usize::from(!complete),
                self.steps.len()
            ))
        }
    }
}
impl BehaviorModule for TaskSequence {
    fn interruption_status(
        &self,
        _context: &crate::InterruptionContext<'_>,
        reason: crate::InterruptionReason,
    ) -> crate::InterruptionStatus {
        if self.terminal.is_some() {
            return crate::InterruptionStatus::Ready;
        }
        // A handoff checkpoints the sequence and the successor resumes it by
        // step name. That is only durable at a step boundary — while an
        // operation is in flight its handle and result belong to this process.
        if reason == crate::InterruptionReason::Handoff {
            return if self.in_flight.is_some() {
                crate::InterruptionStatus::Deferred(
                    "Waiting for the current step operation to finish".into(),
                )
            } else {
                crate::InterruptionStatus::Ready
            };
        }
        let progress = self.progress();
        crate::InterruptionStatus::Deferred(format!(
            "Durable sequence in progress: {} / {} steps",
            progress.completed_steps, progress.total_steps
        ))
    }
    fn non_interruptible(&self) -> bool {
        // An issued operation may hold server-side work (purchases, portal
        // jumps); its deadline pause is what makes the drain observable.
        self.terminal.is_none() && self.in_flight.is_some()
    }
    fn export_state(
        &self,
        now_ms: u64,
        best_effort: bool,
    ) -> Option<crate::ModuleState> {
        let mut issues = Vec::new();
        if self.in_flight.is_some() {
            issues.push("in-flight operation discarded; the step re-issues its request".into());
        }
        let checkpoint = crate::SequenceCheckpoint {
            schema: crate::CHECKPOINT_SCHEMA,
            step_names: self
                .steps
                .iter()
                .map(|step| match step {
                    Step::Operation { name, .. }
                    | Step::Delay { name, .. }
                    | Step::Until { name, .. } => name.clone(),
                })
                .collect(),
            index: self.index,
            current_step: self.progress().current_step,
            attempt: self.attempt,
            entered_elapsed_ms: crate::checkpoint::age_ms(self.entered_at, now_ms),
            retry_in_ms: crate::checkpoint::remaining_ms(Some(self.retry_at), now_ms),
            results: self.results.clone(),
            run_key: self.run_key.clone(),
            terminal: self.terminal.as_ref().map(|output| crate::TerminalOutput {
                state: match output.state {
                    crate::RunState::Completed => crate::TerminalState::Completed,
                    crate::RunState::Failed(_) => crate::TerminalState::Failed,
                    _ => crate::TerminalState::Cancelled,
                },
                status: output.status.clone(),
            }),
        };
        let data = serde_json::to_value(&checkpoint).ok()?;
        let mut state = crate::ModuleState::clean(String::new(), data);
        state.degraded = best_effort || !issues.is_empty();
        state.issues = issues;
        Some(state)
    }
    fn restore_state(
        &mut self,
        state: &crate::ModuleState,
        now_ms: u64,
    ) -> Result<(), crate::RestoreError> {
        state.check_schema()?;
        let checkpoint: crate::SequenceCheckpoint =
            serde_json::from_value(state.data.clone()).map_err(|error| {
                crate::RestoreError::Corrupt(format!("sequence checkpoint: {error}"))
            })?;
        let names: Vec<String> = self
            .steps
            .iter()
            .map(|step| match step {
                Step::Operation { name, .. }
                | Step::Delay { name, .. }
                | Step::Until { name, .. } => name.clone(),
            })
            .collect();
        if checkpoint.step_names != names {
            return Err(crate::RestoreError::ShapeMismatch(format!(
                "checkpoint steps {:?} do not match the rebuilt sequence {:?}",
                checkpoint.step_names, names
            )));
        }
        if checkpoint.index > self.steps.len() {
            return Err(crate::RestoreError::ShapeMismatch(format!(
                "step index {} is out of range ({} steps)",
                checkpoint.index,
                self.steps.len()
            )));
        }
        self.index = checkpoint.index;
        self.attempt = checkpoint.attempt;
        self.entered_at =
            crate::checkpoint::rebase_age(now_ms, checkpoint.entered_elapsed_ms);
        self.retry_at =
            crate::checkpoint::rebase_deadline(now_ms, checkpoint.retry_in_ms).unwrap_or(0);
        self.results = checkpoint.results;
        self.run_key = checkpoint.run_key;
        self.in_flight = None;
        self.terminal = checkpoint.terminal.map(|terminal| ModuleOutput {
            state: match terminal.state {
                crate::TerminalState::Completed => crate::RunState::Completed,
                crate::TerminalState::Failed => crate::RunState::Failed(terminal.status.clone()),
                crate::TerminalState::Cancelled => crate::RunState::Cancelled,
            },
            status: terminal.status,
            plan: None,
        });
        Ok(())
    }
    fn tick(&mut self, context: &mut TickContext<'_>) -> ModuleOutput {
        TaskSequence::tick(self, context)
    }
}
fn milliseconds(duration: Duration) -> u64 {
    duration.as_millis().min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod interruption_tests {
    use super::*;

    #[test]
    fn durable_sequence_defers_until_terminal() {
        let sequence = TaskSequence::new().delay("workflow", Duration::from_secs(1));
        let context = crate::InterruptionContext {
            now_ms: 0,
            snapshot: None,
            game_ready: false,
        };
        assert!(matches!(
            sequence.interruption_status(&context, crate::InterruptionReason::ClientRefresh),
            crate::InterruptionStatus::Deferred(_)
        ));
        let empty = TaskSequence::new();
        assert!(matches!(
            empty.interruption_status(&context, crate::InterruptionReason::ClientRefresh),
            crate::InterruptionStatus::Deferred(_)
        ));
    }
}

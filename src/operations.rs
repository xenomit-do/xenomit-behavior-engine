//! Completion-aware service operations with cooperative cancellation.
use serde_json::Value;
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    thread,
};
use xenomit_game_api::FunctionCall;

#[derive(Clone, Default)]
pub struct CancellationToken {
    local: Arc<AtomicBool>,
    parents: Vec<Arc<AtomicBool>>,
}
impl CancellationToken {
    pub fn cancel(&self) {
        self.local.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.local.load(Ordering::Acquire) || self.parents.iter().any(|p| p.load(Ordering::Acquire))
    }
    pub fn child(&self) -> Self {
        let mut parents = self.parents.clone();
        parents.push(self.local.clone());
        Self {
            local: Arc::new(AtomicBool::new(false)),
            parents,
        }
    }
}

#[derive(Clone, Debug)]
pub struct OperationRequest {
    pub service: String,
    pub input: Value,
    pub requires_game: bool,
    /// Only operations declared idempotent may automatically retry. Services
    /// performing purchases/spins must honor this key before declaring that.
    pub idempotent: bool,
    pub idempotency_key: Option<String>,
}
impl OperationRequest {
    pub fn service(service: impl Into<String>, input: Value) -> Self {
        Self {
            service: service.into(),
            input,
            requires_game: false,
            idempotent: false,
            idempotency_key: None,
        }
    }
    pub fn game_call(call: FunctionCall) -> Self {
        Self {
            service: "game.call".into(),
            input: serde_json::to_value(call).expect("function calls serialize"),
            requires_game: true,
            idempotent: false,
            idempotency_key: None,
        }
    }
    pub fn game_command(command: xenomit_game_api::GameCommand) -> Self {
        Self::service(
            "game.command",
            serde_json::to_value(command).expect("game commands serialize"),
        )
        .requires_game()
    }
    pub fn idempotent(mut self) -> Self {
        self.idempotent = true;
        self
    }
    pub fn requires_game(mut self) -> Self {
        self.requires_game = true;
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct OperationHandle(pub u64);
#[derive(Clone, Debug, PartialEq)]
pub enum OperationPoll {
    Pending,
    Succeeded(Value),
    Failed(String),
}
pub trait OperationHost {
    fn start(&mut self, request: OperationRequest) -> Result<OperationHandle, String>;
    fn poll(&mut self, handle: OperationHandle) -> OperationPoll;
    fn cancel(&mut self, handle: OperationHandle);
    fn release(&mut self, handle: OperationHandle);
}

pub struct ServiceContext {
    pub cancellation: CancellationToken,
    pub idempotency_key: Option<String>,
}
type Service = dyn Fn(ServiceContext, Value) -> Result<Value, String> + Send + Sync;
#[derive(Default)]
pub struct ServiceRegistryBuilder {
    services: BTreeMap<String, Arc<Service>>,
}
impl ServiceRegistryBuilder {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn register<F>(&mut self, name: impl Into<String>, service: F) -> Result<&mut Self, String>
    where
        F: Fn(ServiceContext, Value) -> Result<Value, String> + Send + Sync + 'static,
    {
        let name = name.into();
        if name.is_empty() || self.services.contains_key(&name) {
            return Err(format!("invalid/duplicate service {name}"));
        }
        self.services.insert(name, Arc::new(service));
        Ok(self)
    }
    pub fn build(self) -> ServiceRegistry {
        ServiceRegistry {
            services: self.services,
        }
    }
}
#[derive(Clone, Default)]
pub struct ServiceRegistry {
    services: BTreeMap<String, Arc<Service>>,
}
impl ServiceRegistry {
    pub fn contains(&self, name: &str) -> bool {
        self.services.contains_key(name)
    }
    pub fn with_game_client(mut self, client: Arc<dyn xenomit_game_api::GameClient>) -> Self {
        let command_client = client.clone();
        self.services.insert(
            "game.command".into(),
            Arc::new(move |context, input| {
                if context.cancellation.is_cancelled() {
                    return Err("Cancelled".into());
                }
                let command = serde_json::from_value(input)
                    .map_err(|e| format!("invalid game command: {e}"))?;
                command_client
                    .game_command(&command)
                    .map(|_| Value::Null)
                    .map_err(|e| e.to_string())
            }),
        );
        self.services.insert(
            "game.call".into(),
            Arc::new(move |context, input| {
                if context.cancellation.is_cancelled() {
                    return Err("Cancelled".into());
                }
                let call =
                    serde_json::from_value(input).map_err(|e| format!("invalid game call: {e}"))?;
                client
                    .call_function(&call)
                    .and_then(|result| {
                        serde_json::to_value(result)
                            .map_err(|e| xenomit_game_api::ApiError::Protocol(e.to_string()))
                    })
                    .map_err(|e| e.to_string())
            }),
        );
        self
    }
}
struct Job {
    receiver: mpsc::Receiver<Result<Value, String>>,
    token: CancellationToken,
    result: Option<OperationPoll>,
}
/// The module worker never blocks on service work. Capacity includes cancelled
/// jobs until they actually finish, preventing unbounded runaway retries.
pub struct ThreadedOperationHost {
    services: ServiceRegistry,
    cancellation: CancellationToken,
    jobs: BTreeMap<OperationHandle, Job>,
    retired: Vec<Job>,
    capacity: usize,
}
impl ThreadedOperationHost {
    pub fn new(services: ServiceRegistry, cancellation: CancellationToken) -> Self {
        Self {
            services,
            cancellation,
            jobs: BTreeMap::new(),
            retired: Vec::new(),
            capacity: 32,
        }
    }
    fn reap(&mut self) {
        self.retired
            .retain(|j| matches!(j.receiver.try_recv(), Err(mpsc::TryRecvError::Empty)));
    }
}
impl OperationHost for ThreadedOperationHost {
    fn start(&mut self, request: OperationRequest) -> Result<OperationHandle, String> {
        self.reap();
        if self.cancellation.is_cancelled() {
            return Err("Cancelled".into());
        }
        if self.jobs.len() + self.retired.len() >= self.capacity {
            return Err("service operation capacity exhausted".into());
        }
        let service = self
            .services
            .services
            .get(&request.service)
            .cloned()
            .ok_or_else(|| format!("service {} is unavailable", request.service))?;
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let handle = OperationHandle(NEXT.fetch_add(1, Ordering::Relaxed));
        let token = self.cancellation.child();
        let worker_token = token.clone();
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name(format!("behavior-operation-{}", handle.0))
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    service(
                        ServiceContext {
                            cancellation: worker_token,
                            idempotency_key: request.idempotency_key,
                        },
                        request.input,
                    )
                }))
                .unwrap_or_else(|_| Err("service panicked".into()));
                let _ = sender.send(result);
            })
            .map_err(|e| e.to_string())?;
        self.jobs.insert(
            handle,
            Job {
                receiver,
                token,
                result: None,
            },
        );
        Ok(handle)
    }
    fn poll(&mut self, handle: OperationHandle) -> OperationPoll {
        let Some(job) = self.jobs.get_mut(&handle) else {
            return OperationPoll::Failed("unknown operation handle".into());
        };
        if let Some(result) = &job.result {
            return result.clone();
        }
        let result = match job.receiver.try_recv() {
            Ok(Ok(value)) => OperationPoll::Succeeded(value),
            Ok(Err(error)) => OperationPoll::Failed(error),
            Err(mpsc::TryRecvError::Empty) => return OperationPoll::Pending,
            Err(mpsc::TryRecvError::Disconnected) => {
                OperationPoll::Failed("service worker disconnected".into())
            }
        };
        job.result = Some(result.clone());
        result
    }
    fn cancel(&mut self, handle: OperationHandle) {
        if let Some(job) = self.jobs.get(&handle) {
            job.token.cancel();
        }
    }
    fn release(&mut self, handle: OperationHandle) {
        if let Some(job) = self.jobs.remove(&handle) {
            if job.result.is_none() {
                job.token.cancel();
                self.retired.push(job);
            }
        }
        self.reap();
    }
}
impl Drop for ThreadedOperationHost {
    fn drop(&mut self) {
        for job in self.jobs.values().chain(&self.retired) {
            job.token.cancel();
        }
    }
}

//! Bounded A* with continuous collision checks and line-of-sight smoothing.
//! All obstacle geometry is in world coordinates; Navigation queries honor
//! the planner's affine coordinate transform.
use crate::{CoordinateTransform, Navigation, Point};
use std::{cmp::Ordering, collections::BinaryHeap};
use xenomit_game_api::{GameEntity, GameSnapshot, Obstacle, ObstacleShape};

pub const PATH_CLEARANCE: f64 = 60.0;
pub struct ObstacleNavigation<'a> {
    width: f64,
    height: f64,
    obstacles: Vec<PreparedObstacle<'a>>,
    transform: CoordinateTransform,
    available: bool,
    bounded: bool,
}
impl<'a> ObstacleNavigation<'a> {
    pub fn from_snapshot(snapshot: &'a GameSnapshot, transform: CoordinateTransform) -> Self {
        let map_id = snapshot.map.as_ref().and_then(|m| m.id);
        Self {
            width: snapshot
                .map
                .as_ref()
                .and_then(|m| m.width)
                .unwrap_or(21_000.0),
            height: snapshot
                .map
                .as_ref()
                .and_then(|m| m.height)
                .unwrap_or(13_100.0),
            obstacles: if snapshot.navigation.map_id == map_id {
                snapshot
                    .navigation
                    .obstacles
                    .iter()
                    .map(PreparedObstacle::new)
                    .collect()
            } else {
                vec![]
            },
            available: snapshot.navigation.map_id.is_none()
                || (snapshot.navigation.map_id == map_id && snapshot.navigation.geometry_ready),
            transform,
            bounded: snapshot
                .map
                .as_ref()
                .and_then(|m| m.width.zip(m.height))
                .is_some(),
        }
    }
    pub fn new(width: f64, height: f64, obstacles: &'a [Obstacle]) -> Self {
        Self {
            width,
            height,
            obstacles: obstacles.iter().map(PreparedObstacle::new).collect(),
            transform: CoordinateTransform::default(),
            available: true,
            bounded: true,
        }
    }
    fn raw(&self, point: Point) -> Point {
        let (x, y) = self.transform.inverse_apply(point.x, point.y);
        Point { x, y }
    }
    fn transformed(&self, point: Point) -> Point {
        let (x, y) = self.transform.apply(point.x, point.y);
        Point { x, y }
    }
    fn valid_bounds(&self) -> bool {
        self.available
            && self.width.is_finite()
            && self.height.is_finite()
            && self.width > 0.0
            && self.height > 0.0
    }
    fn free(&self, point: Point, clearance: f64) -> bool {
        self.valid_bounds()
            && point.x.is_finite()
            && point.y.is_finite()
            && (!self.bounded
                || ((0.0..=self.width).contains(&point.x)
                    && (0.0..=self.height).contains(&point.y)))
            && self.obstacles.iter().all(|o| !blocked(o, point, clearance))
    }
    fn clear(&self, from: Point, to: Point, clearance: f64) -> bool {
        self.free(from, clearance)
            && self.free(to, clearance)
            && self
                .obstacles
                .iter()
                .all(|o| !crosses(o, from, to, clearance))
    }
    pub fn segment_clear(&self, from: Point, to: Point) -> bool {
        self.clear(self.raw(from), self.raw(to), PATH_CLEARANCE)
    }
    /// Distance to nearby walkable geometry, used for boxes/NPCs in obstacles.
    pub fn nearest_point(&self, point: Point) -> Option<Point> {
        self.nearest_raw(self.raw(point))
            .map(|p| self.transformed(p))
    }
    fn nearest_raw(&self, point: Point) -> Option<Point> {
        let started = std::time::Instant::now();
        if self.free(point, PATH_CLEARANCE) {
            return Some(point);
        }
        if !self.valid_bounds() {
            return None;
        }
        for ring in 1..=40 {
            if started.elapsed() > std::time::Duration::from_millis(50) {
                return None;
            }
            let radius = f64::from(ring) * 25.0;
            for angle in 0..64 {
                let radians = f64::from(angle) * std::f64::consts::TAU / 64.0;
                let candidate = Point {
                    x: point.x + radians.cos() * radius,
                    y: point.y + radians.sin() * radius,
                };
                if self.free(candidate, PATH_CLEARANCE) {
                    return Some(candidate);
                }
            }
        }
        None
    }
    /// Return waypoints after `from`. A blocked destination is projected to a
    /// nearby reachable point. Never emit a segment cutting an obstacle.
    pub fn find_path(&self, from: Point, destination: Point) -> Result<Vec<Point>, String> {
        let started = std::time::Instant::now();
        if !self.valid_bounds() {
            return Err("Waiting for current-map obstacle geometry".into());
        }
        let original_from = self.raw(from);
        if !self.free(original_from, 0.0) {
            return Err("Hero is inside a no-access zone; no safe route".into());
        }
        let from = self
            .nearest_raw(original_from)
            .ok_or("No safe starting point")?;
        if !self.clear(original_from, from, 0.0) {
            return Err("Cannot safely leave the obstacle margin".into());
        }
        let to = self
            .nearest_raw(self.raw(destination))
            .ok_or("Destination has no reachable approach")?;
        let mut prefix = Vec::new();
        if distance(original_from, from) > 1.0 {
            prefix.push(from);
        }
        if self.clear(from, to, PATH_CLEARANCE) {
            prefix.push(to);
            return Ok(prefix.into_iter().map(|p| self.transformed(p)).collect());
        }
        let cell = 100.0_f64.max(self.width / 255.0).max(self.height / 255.0);
        let columns = (self.width / cell).ceil() as usize + 1;
        let rows = (self.height / cell).ceil() as usize + 1;
        let count = columns * rows;
        if count > 65_536 {
            return Err("Map is too large for bounded path search".into());
        }
        let point_at = |index: usize| Point {
            x: ((index % columns) as f64 * cell).min(self.width),
            y: ((index / columns) as f64 * cell).min(self.height),
        };
        let endpoint = |point: Point| -> Option<usize> {
            let x = (point.x / cell).round() as isize;
            let y = (point.y / cell).round() as isize;
            (-3..=3)
                .flat_map(|dx| (-3..=3).map(move |dy| (x + dx, y + dy)))
                .filter(|(x, y)| {
                    *x >= 0 && *y >= 0 && (*x as usize) < columns && (*y as usize) < rows
                })
                .map(|(x, y)| y as usize * columns + x as usize)
                .filter(|index| self.clear(point, point_at(*index), PATH_CLEARANCE))
                .min_by(|a, b| {
                    distance(point, point_at(*a)).total_cmp(&distance(point, point_at(*b)))
                })
        };
        let start = endpoint(from).ok_or("No connected path from hero")?;
        let goal = endpoint(to).ok_or("No connected path to destination")?;
        let mut costs = vec![f64::INFINITY; count];
        let mut parents = vec![usize::MAX; count];
        let mut frontier = BinaryHeap::new();
        costs[start] = 0.0;
        frontier.push(Node {
            index: start,
            score: distance(point_at(start), point_at(goal)),
            cost: 0.0,
        });
        let mut found = false;
        let mut expanded = 0;
        while let Some(node) = frontier.pop() {
            if node.cost > costs[node.index] {
                continue;
            }
            expanded += 1;
            if expanded % 64 == 0 && started.elapsed() > std::time::Duration::from_millis(200) {
                return Err("Path search exceeded its time budget; movement paused".into());
            }
            if node.index == goal {
                found = true;
                break;
            }
            let x = (node.index % columns) as isize;
            let y = (node.index / columns) as isize;
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = x + dx;
                    let ny = y + dy;
                    if nx < 0 || ny < 0 || nx as usize >= columns || ny as usize >= rows {
                        continue;
                    }
                    let next = ny as usize * columns + nx as usize;
                    let new_cost = node.cost + distance(point_at(node.index), point_at(next));
                    if new_cost >= costs[next]
                        || !self.clear(point_at(node.index), point_at(next), PATH_CLEARANCE)
                    {
                        continue;
                    }
                    costs[next] = new_cost;
                    parents[next] = node.index;
                    frontier.push(Node {
                        index: next,
                        cost: new_cost,
                        score: new_cost + distance(point_at(next), point_at(goal)),
                    });
                }
            }
        }
        if !found {
            return Err("No route around the current obstacles".into());
        }
        let mut path = vec![to];
        let mut index = goal;
        loop {
            path.push(point_at(index));
            if index == start {
                break;
            }
            index = parents[index];
        }
        path.reverse();
        let mut current = from;
        let mut offset = 0;
        while offset < path.len() {
            let next = (offset..path.len())
                .rev()
                .find(|i| self.clear(current, path[*i], PATH_CLEARANCE))
                .ok_or("Path changed during smoothing")?;
            current = path[next];
            prefix.push(current);
            offset = next + 1;
        }
        Ok(prefix.into_iter().map(|p| self.transformed(p)).collect())
    }
}
impl Navigation for ObstacleNavigation<'_> {
    fn can_move(&self, point: Point) -> bool {
        self.free(self.raw(point), PATH_CLEARANCE)
    }
    fn closest_distance(&self, _: &GameEntity, point: Point) -> f64 {
        self.nearest_point(point)
            .map_or(f64::INFINITY, |near| distance(point, near))
    }
}
#[derive(Clone, Copy)]
struct Node {
    index: usize,
    score: f64,
    cost: f64,
}
impl PartialEq for Node {
    fn eq(&self, other: &Self) -> bool {
        self.index == other.index && self.score == other.score
    }
}
impl Eq for Node {}
impl Ord for Node {
    fn cmp(&self, other: &Self) -> Ordering {
        other
            .score
            .total_cmp(&self.score)
            .then_with(|| other.index.cmp(&self.index))
    }
}
impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
fn distance(a: Point, b: Point) -> f64 {
    (a.x - b.x).hypot(a.y - b.y)
}
fn convert(p: xenomit_game_api::Point) -> Point {
    Point { x: p.x, y: p.y }
}
fn vertices(shape: &ObstacleShape) -> Vec<Point> {
    match shape {
        ObstacleShape::Rectangle { bounds: b } => vec![
            Point { x: b.x, y: b.y },
            Point {
                x: b.x + b.width,
                y: b.y,
            },
            Point {
                x: b.x + b.width,
                y: b.y + b.height,
            },
            Point {
                x: b.x,
                y: b.y + b.height,
            },
        ],
        ObstacleShape::Polygon { points } => points.iter().copied().map(convert).collect(),
        _ => Vec::new(),
    }
}
struct PreparedObstacle<'a> {
    source: &'a Obstacle,
    vertices: Vec<Point>,
    bounds: (f64, f64, f64, f64),
    valid: bool,
}
impl<'a> PreparedObstacle<'a> {
    fn new(source: &'a Obstacle) -> Self {
        let vertices = vertices(&source.shape);
        let bounds = if let ObstacleShape::Circle { center, radius } = source.shape {
            (
                center.x - radius,
                center.y - radius,
                center.x + radius,
                center.y + radius,
            )
        } else {
            vertices.iter().fold(
                (
                    f64::INFINITY,
                    f64::INFINITY,
                    f64::NEG_INFINITY,
                    f64::NEG_INFINITY,
                ),
                |bounds, p| {
                    (
                        bounds.0.min(p.x),
                        bounds.1.min(p.y),
                        bounds.2.max(p.x),
                        bounds.3.max(p.y),
                    )
                },
            )
        };
        let valid = [bounds.0, bounds.1, bounds.2, bounds.3]
            .iter()
            .all(|n| n.is_finite())
            && bounds.0 < bounds.2
            && bounds.1 < bounds.3
            && match &source.shape {
                ObstacleShape::Circle { radius, .. } => *radius > 0.0,
                ObstacleShape::Rectangle { bounds } => bounds.width > 0.0 && bounds.height > 0.0,
                ObstacleShape::Polygon { points } => {
                    (3..=1024).contains(&points.len())
                        && points.iter().all(|p| p.x.is_finite() && p.y.is_finite())
                }
            };
        Self {
            source,
            vertices,
            bounds,
            valid,
        }
    }
    fn distant(&self, from: Point, to: Point, clearance: f64) -> bool {
        !self.source.inverted
            && (from.x.max(to.x) < self.bounds.0 - clearance
                || from.x.min(to.x) > self.bounds.2 + clearance
                || from.y.max(to.y) < self.bounds.1 - clearance
                || from.y.min(to.y) > self.bounds.3 + clearance)
    }
}
fn signed_distance(obstacle: &PreparedObstacle<'_>, p: Point) -> f64 {
    let shape = &obstacle.source.shape;
    if let ObstacleShape::Circle { center, radius } = shape {
        if !radius.is_finite() || *radius < 0.0 {
            return f64::NEG_INFINITY;
        }
        return distance(convert(*center), p) - radius;
    }
    if let ObstacleShape::Rectangle { bounds } = shape {
        if bounds.width <= 0.0 || bounds.height <= 0.0 {
            return f64::NEG_INFINITY;
        }
    }
    let points = &obstacle.vertices;
    if points.len() < 3 || points.iter().any(|p| !p.x.is_finite() || !p.y.is_finite()) {
        return f64::NEG_INFINITY;
    }
    let mut inside = false;
    let mut nearest = f64::INFINITY;
    for index in 0..points.len() {
        let a = points[index];
        let b = points[(index + 1) % points.len()];
        nearest = nearest.min(point_segment_distance(p, a, b));
        if (a.y > p.y) != (b.y > p.y) && p.x < (b.x - a.x) * (p.y - a.y) / (b.y - a.y) + a.x {
            inside = !inside;
        }
    }
    if inside { -nearest } else { nearest }
}
fn blocked(obstacle: &PreparedObstacle<'_>, point: Point, clearance: f64) -> bool {
    if !obstacle.valid {
        return true;
    }
    if obstacle.distant(point, point, clearance) {
        return false;
    }
    let d = signed_distance(obstacle, point);
    if !d.is_finite() {
        return true;
    }
    if obstacle.source.inverted {
        d >= -clearance
    } else {
        d <= clearance
    }
}
fn crosses(obstacle: &PreparedObstacle<'_>, from: Point, to: Point, clearance: f64) -> bool {
    if !obstacle.valid {
        return true;
    }
    if obstacle.distant(from, to, clearance) {
        return false;
    }
    if let ObstacleShape::Circle { center, radius } = &obstacle.source.shape {
        // A circle's interior is convex, so two safe interior endpoints suffice.
        return !obstacle.source.inverted
            && point_segment_distance(convert(*center), from, to) <= radius + clearance;
    }
    let points = &obstacle.vertices;
    for index in 0..points.len() {
        let a = points[index];
        let b = points[(index + 1) % points.len()];
        if intersects(from, to, a, b) || segment_distance(from, to, a, b) <= clearance {
            return true;
        }
    }
    false
}
fn point_segment_distance(point: Point, from: Point, to: Point) -> f64 {
    let dx = to.x - from.x;
    let dy = to.y - from.y;
    let length = dx * dx + dy * dy;
    let fraction = if length > 0.0 {
        ((point.x - from.x) * dx + (point.y - from.y) * dy) / length
    } else {
        0.0
    }
    .clamp(0.0, 1.0);
    distance(
        point,
        Point {
            x: from.x + dx * fraction,
            y: from.y + dy * fraction,
        },
    )
}
fn segment_distance(a: Point, b: Point, c: Point, d: Point) -> f64 {
    point_segment_distance(a, c, d)
        .min(point_segment_distance(b, c, d))
        .min(point_segment_distance(c, a, b))
        .min(point_segment_distance(d, a, b))
}
fn intersects(a: Point, b: Point, c: Point, d: Point) -> bool {
    let cross =
        |a: Point, b: Point, c: Point| (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
    let c1 = cross(a, b, c);
    let c2 = cross(a, b, d);
    let c3 = cross(c, d, a);
    let c4 = cross(c, d, b);
    if c1.abs() < 1e-8 && c2.abs() < 1e-8 {
        return segment_distance(a, b, c, d) < 1e-8;
    }
    c1 * c2 <= 0.0 && c3 * c4 <= 0.0
}

/// Retained route follower. Replans when geometry/map/goal changes or progress
/// stalls; checks every segment against the latest snapshot before issuing it.
#[derive(Default)]
pub struct MovementRouter {
    map: Option<i32>,
    goal: Option<Point>,
    endpoint: Option<Point>,
    obstacles: Vec<Obstacle>,
    waypoints: std::collections::VecDeque<Point>,
    progress: Option<(Point, u64)>,
}
impl MovementRouter {
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn next(
        &mut self,
        snapshot: &GameSnapshot,
        from: Point,
        goal: Point,
        now_ms: u64,
    ) -> Result<Option<Point>, String> {
        let navigation =
            ObstacleNavigation::from_snapshot(snapshot, CoordinateTransform::default());
        let map = snapshot.map.as_ref().and_then(|m| m.id);
        let stalled = self.progress.is_some_and(|(position, at)| {
            distance(from, position) < 30.0 && now_ms.saturating_sub(at) > 5_000
        });
        if self
            .progress
            .is_none_or(|(position, _)| distance(from, position) >= 30.0)
        {
            self.progress = Some((from, now_ms));
        }
        let replan = self.map != map
            || self
                .goal
                .is_none_or(|previous| distance(previous, goal) > 50.0)
            || self.obstacles != snapshot.navigation.obstacles
            || stalled
            || (self.waypoints.is_empty()
                && self.endpoint.is_some_and(|end| distance(from, end) > 35.0))
            || self
                .waypoints
                .front()
                .is_some_and(|next| !navigation.segment_clear(from, *next));
        if replan {
            self.waypoints.clear();
            self.waypoints.extend(navigation.find_path(from, goal)?);
            self.endpoint = self.waypoints.back().copied();
            self.map = map;
            self.goal = Some(goal);
            self.obstacles = snapshot.navigation.obstacles.clone();
            self.progress = Some((from, now_ms));
        }
        while self
            .waypoints
            .front()
            .is_some_and(|next| distance(from, *next) < 35.0)
        {
            self.waypoints.pop_front();
        }
        // Skip already visible corners, but never cut across a new obstacle.
        if let Some(index) = self
            .waypoints
            .iter()
            .rposition(|next| navigation.segment_clear(from, *next))
        {
            for _ in 0..index {
                self.waypoints.pop_front();
            }
        }
        let next = self.waypoints.front().copied();
        if let Some(next) = next {
            if !navigation.segment_clear(from, next) {
                // Permit the short initial exit from clearance, never an obstacle.
                let route = navigation.find_path(from, goal)?;
                self.waypoints = route.into();
                return Ok(self.waypoints.front().copied());
            }
        }
        Ok(next)
    }
}

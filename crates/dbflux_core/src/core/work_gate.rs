//! Admission control for database work: a bounded number of slots per lane,
//! and a FIFO queue in front of them.
//!
//! A unit of work asks for a slot with [`WorkGate::admit`] and gets a
//! [`WorkTicket`], a future that resolves to a [`WorkPermit`] once a slot is
//! free. The slot is held for as long as the permit lives.
//!
//! Nesting rule: work never asks for a slot while it holds a permit. A permit
//! is moved into the one blocking call (or the one task) it covers, and any
//! follow-up work, such as the re-read after a save or the row count after a
//! page, is admitted only after that call returned and dropped the permit.
//! Holding a permit while awaiting another ticket of the same lane can
//! deadlock once every slot is held by such a waiter, so the rule is what
//! keeps the queue deadlock free.

use std::collections::{HashMap, VecDeque};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, MutexGuard};
use std::task::{Context, Poll, Waker};

use uuid::Uuid;

use super::task::{CancelToken, TaskId};

pub const DEFAULT_MAX_CONCURRENT_QUERIES: usize = 8;
pub const DEFAULT_MAX_QUEUED_QUERIES: usize = 50;
pub const DEFAULT_MAX_CONNECTION_TASKS: usize = 8;

/// The pool of slots a unit of work competes for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WorkLane {
    /// Queries and commands sent over one connection profile's connection.
    Queries(Uuid),
    /// Connection and schema tasks (connects, disconnects, schema loads and
    /// refreshes, key scans), limited across all connections together.
    ConnectionTasks,
}

impl WorkLane {
    pub fn profile_id(&self) -> Option<Uuid> {
        match self {
            WorkLane::Queries(profile_id) => Some(*profile_id),
            WorkLane::ConnectionTasks => None,
        }
    }
}

/// Interactive work (an explicit Run in the SQL editor, a Save) is served
/// before background work (page loads, counts, lookups) that waits in the
/// same lane; within a level the queue is FIFO.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkPriority {
    Interactive,
    Background,
}

/// Why a ticket resolved without a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkRefusal {
    /// The work was cancelled while it waited: its token, its scope, a
    /// "Clear queue" or a disconnect.
    Cancelled,
    /// The queue was full: either when the work arrived, or later, when
    /// interactive work took its place. `waiting` is the queue length then.
    QueueFull { waiting: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorkLimits {
    pub max_concurrent_queries: usize,
    pub max_queued_queries: usize,
    pub max_connection_tasks: usize,
}

impl Default for WorkLimits {
    fn default() -> Self {
        Self {
            max_concurrent_queries: DEFAULT_MAX_CONCURRENT_QUERIES,
            max_queued_queries: DEFAULT_MAX_QUEUED_QUERIES,
            max_connection_tasks: DEFAULT_MAX_CONNECTION_TASKS,
        }
    }
}

impl WorkLimits {
    fn running_limit(&self, lane: WorkLane) -> usize {
        let limit = match lane {
            WorkLane::Queries(_) => self.max_concurrent_queries,
            WorkLane::ConnectionTasks => self.max_connection_tasks,
        };

        limit.max(1)
    }

    /// Connection tasks queue without a cap: they are few, and each one
    /// stands for an explicit click (connect, expand, refresh).
    fn queue_cap(&self, lane: WorkLane) -> usize {
        match lane {
            WorkLane::Queries(_) => self.max_queued_queries.max(1),
            WorkLane::ConnectionTasks => usize::MAX,
        }
    }
}

pub struct WorkRequest {
    pub lane: WorkLane,
    pub priority: WorkPriority,
    pub task_id: Option<TaskId>,
    /// Cancelling this token while the work waits takes it out of the queue.
    pub cancel: CancelToken,
    /// The lifetime of whatever asked for the work (a tab); cancelling it
    /// drops the work as well.
    pub scope: Option<CancelToken>,
}

impl WorkRequest {
    pub fn new(lane: WorkLane, priority: WorkPriority, cancel: CancelToken) -> Self {
        Self {
            lane,
            priority,
            task_id: None,
            cancel,
            scope: None,
        }
    }

    pub fn with_task(mut self, task_id: TaskId) -> Self {
        self.task_id = Some(task_id);
        self
    }

    pub fn with_scope(mut self, scope: Option<CancelToken>) -> Self {
        self.scope = scope;
        self
    }
}

/// A queued unit of work, oldest first within its priority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitingWork {
    pub ticket: u64,
    pub priority: WorkPriority,
    pub task_id: Option<TaskId>,
}

#[derive(Clone, Default)]
pub struct WorkGate {
    shared: Arc<Mutex<GateState>>,
}

#[derive(Default)]
struct GateState {
    limits: WorkLimits,
    lanes: HashMap<WorkLane, LaneState>,
    next_ticket: u64,
}

#[derive(Default)]
struct LaneState {
    running: usize,
    interactive: VecDeque<u64>,
    background: VecDeque<u64>,
    waiters: HashMap<u64, Waiter>,
}

struct Waiter {
    priority: WorkPriority,
    task_id: Option<TaskId>,
    cancel: CancelToken,
    scope: Option<CancelToken>,
    outcome: Outcome,
    waker: Option<Waker>,
}

impl Waiter {
    fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled() || self.scope.as_ref().is_some_and(CancelToken::is_cancelled)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Outcome {
    Waiting,
    Granted,
    Refused(WorkRefusal),
}

impl LaneState {
    fn waiting_ids(&self) -> impl Iterator<Item = &u64> {
        self.interactive.iter().chain(self.background.iter())
    }

    /// Queued work that can still run: cancelled waiters stay in the deques
    /// until their future is polled or a slot skips over them.
    fn live_waiting(&self) -> usize {
        self.waiting_ids()
            .filter(|id| {
                self.waiters
                    .get(id)
                    .is_some_and(|waiter| !waiter.is_cancelled())
            })
            .count()
    }

    fn unqueue(&mut self, id: u64) {
        self.interactive.retain(|queued| *queued != id);
        self.background.retain(|queued| *queued != id);
    }

    fn resolve(&mut self, id: u64, outcome: Outcome, wake: &mut Vec<Waker>) {
        if let Some(waiter) = self.waiters.get_mut(&id) {
            waiter.outcome = outcome;
            if let Some(waker) = waiter.waker.take() {
                wake.push(waker);
            }
        }
    }

    /// Hands free slots to queued work, interactive first.
    fn promote(&mut self, limit: usize, wake: &mut Vec<Waker>) {
        while self.running < limit {
            let Some(id) = self
                .interactive
                .pop_front()
                .or_else(|| self.background.pop_front())
            else {
                return;
            };

            let cancelled = self
                .waiters
                .get(&id)
                .is_none_or(|waiter| waiter.is_cancelled());

            if cancelled {
                self.resolve(id, Outcome::Refused(WorkRefusal::Cancelled), wake);
                continue;
            }

            self.running += 1;
            self.resolve(id, Outcome::Granted, wake);
        }
    }

    fn is_idle(&self) -> bool {
        self.running == 0 && self.waiters.is_empty()
    }
}

fn wake_all(wakers: Vec<Waker>) {
    for waker in wakers {
        waker.wake();
    }
}

impl WorkGate {
    pub fn new(limits: WorkLimits) -> Self {
        let gate = Self::default();
        gate.lock().limits = limits;
        gate
    }

    fn lock(&self) -> MutexGuard<'_, GateState> {
        match self.shared.lock() {
            Ok(state) => state,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    pub fn limits(&self) -> WorkLimits {
        self.lock().limits
    }

    /// A raised limit starts queued work right away; a lowered one lets the
    /// running work finish and holds back new work until it fits.
    pub fn set_limits(&self, limits: WorkLimits) {
        let mut wake = Vec::new();
        {
            let mut state = self.lock();
            state.limits = limits;

            let GateState { limits, lanes, .. } = &mut *state;
            for (lane, lane_state) in lanes.iter_mut() {
                lane_state.promote(limits.running_limit(*lane), &mut wake);
            }
        }
        wake_all(wake);
    }

    /// Asks for a slot. The ticket is already resolved when a slot is free
    /// and nothing waits ahead, or when the queue is full; otherwise the work
    /// is queued and the ticket resolves when it reaches a slot.
    ///
    /// On a full queue, interactive work takes the place of the oldest
    /// queued background work, which resolves with
    /// [`WorkRefusal::QueueFull`]; background work, or interactive work on a
    /// queue that holds only interactive work, is refused the same way.
    pub fn admit(&self, request: WorkRequest) -> WorkTicket {
        let mut wake = Vec::new();
        let ticket = {
            let mut state = self.lock();
            let limit = state.limits.running_limit(request.lane);
            let cap = state.limits.queue_cap(request.lane);
            state.next_ticket += 1;
            let id = state.next_ticket;
            let lane_state = state.lanes.entry(request.lane).or_default();

            let cancelled = request.cancel.is_cancelled()
                || request
                    .scope
                    .as_ref()
                    .is_some_and(CancelToken::is_cancelled);

            if cancelled {
                TicketState::Resolved(Err(WorkRefusal::Cancelled))
            } else if lane_state.running < limit && lane_state.live_waiting() == 0 {
                lane_state.running += 1;
                TicketState::Resolved(Ok(()))
            } else {
                let waiting = lane_state.live_waiting();
                let mut admitted = waiting < cap;

                if !admitted && request.priority == WorkPriority::Interactive {
                    while let Some(evicted) = lane_state.background.pop_front() {
                        let live = lane_state
                            .waiters
                            .get(&evicted)
                            .is_some_and(|waiter| !waiter.is_cancelled());
                        let refusal = if live {
                            WorkRefusal::QueueFull { waiting }
                        } else {
                            WorkRefusal::Cancelled
                        };
                        lane_state.resolve(evicted, Outcome::Refused(refusal), &mut wake);

                        if live {
                            admitted = true;
                            break;
                        }
                    }
                }

                if admitted {
                    lane_state.waiters.insert(
                        id,
                        Waiter {
                            priority: request.priority,
                            task_id: request.task_id,
                            cancel: request.cancel.clone(),
                            scope: request.scope.clone(),
                            outcome: Outcome::Waiting,
                            waker: None,
                        },
                    );
                    match request.priority {
                        WorkPriority::Interactive => lane_state.interactive.push_back(id),
                        WorkPriority::Background => lane_state.background.push_back(id),
                    }
                    TicketState::Queued(id)
                } else {
                    TicketState::Resolved(Err(WorkRefusal::QueueFull { waiting }))
                }
            }
        };
        wake_all(wake);

        WorkTicket {
            gate: self.clone(),
            lane: request.lane,
            cancel: request.cancel,
            scope: request.scope,
            listener: NEXT_LISTENER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            state: ticket,
        }
    }

    fn release(&self, lane: WorkLane) {
        let mut wake = Vec::new();
        {
            let mut state = self.lock();
            let limit = state.limits.running_limit(lane);
            if let Some(lane_state) = state.lanes.get_mut(&lane) {
                lane_state.running = lane_state.running.saturating_sub(1);
                lane_state.promote(limit, &mut wake);

                if lane_state.is_idle() {
                    state.lanes.remove(&lane);
                }
            }
        }
        wake_all(wake);
    }

    /// Cancels every unit of work waiting in `lane` and returns the tasks
    /// they belonged to. Running work keeps its slot.
    pub fn clear_queue(&self, lane: WorkLane) -> Vec<TaskId> {
        let mut wake = Vec::new();
        let mut task_ids = Vec::new();
        {
            let mut state = self.lock();
            if let Some(lane_state) = state.lanes.get_mut(&lane) {
                let queued: Vec<u64> = lane_state.waiting_ids().copied().collect();
                lane_state.interactive.clear();
                lane_state.background.clear();

                for id in queued {
                    if let Some(task_id) = lane_state.waiters.get(&id).and_then(|w| w.task_id) {
                        task_ids.push(task_id);
                    }
                    lane_state.resolve(id, Outcome::Refused(WorkRefusal::Cancelled), &mut wake);
                }
            }
        }
        wake_all(wake);
        task_ids
    }

    /// Records the task that reports queued work admitted without one.
    pub fn attach_task(&self, ticket: &WorkTicket, task_id: TaskId) {
        let TicketState::Queued(id) = ticket.state else {
            return;
        };

        if let Some(waiter) = self
            .lock()
            .lanes
            .get_mut(&ticket.lane)
            .and_then(|lane_state| lane_state.waiters.get_mut(&id))
        {
            waiter.task_id = Some(task_id);
        }
    }

    pub fn running_count(&self, lane: WorkLane) -> usize {
        self.lock()
            .lanes
            .get(&lane)
            .map_or(0, |lane_state| lane_state.running)
    }

    pub fn waiting_count(&self, lane: WorkLane) -> usize {
        self.lock()
            .lanes
            .get(&lane)
            .map_or(0, LaneState::live_waiting)
    }

    /// The queued work of `lane` in the order it will start.
    pub fn waiting(&self, lane: WorkLane) -> Vec<WaitingWork> {
        let state = self.lock();
        let Some(lane_state) = state.lanes.get(&lane) else {
            return Vec::new();
        };

        lane_state
            .waiting_ids()
            .filter_map(|id| {
                let waiter = lane_state.waiters.get(id)?;
                (!waiter.is_cancelled()).then_some(WaitingWork {
                    ticket: *id,
                    priority: waiter.priority,
                    task_id: waiter.task_id,
                })
            })
            .collect()
    }

    /// Connection profiles with queued queries, with how many wait on each.
    pub fn queued_query_lanes(&self) -> Vec<(Uuid, usize)> {
        let state = self.lock();
        let mut lanes: Vec<(Uuid, usize)> = state
            .lanes
            .iter()
            .filter_map(|(lane, lane_state)| {
                let profile_id = lane.profile_id()?;
                let waiting = lane_state.live_waiting();
                (waiting > 0).then_some((profile_id, waiting))
            })
            .collect();
        lanes.sort();
        lanes
    }
}

enum TicketState {
    Queued(u64),
    /// `Ok(())` holds a slot already counted as running.
    Resolved(Result<(), WorkRefusal>),
    Done,
}

/// Resolves to a slot, or to the reason there is none. Dropping a ticket
/// before it resolves leaves the queue; dropping one that was granted a slot
/// but never polled hands the slot on.
pub struct WorkTicket {
    gate: WorkGate,
    lane: WorkLane,
    cancel: CancelToken,
    scope: Option<CancelToken>,
    state: TicketState,
    /// Names this ticket's waker on its tokens, so it can be taken off them.
    listener: u64,
}

static NEXT_LISTENER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl WorkTicket {
    /// The work did not get a slot when it arrived and waits in the queue.
    pub fn is_queued(&self) -> bool {
        matches!(self.state, TicketState::Queued(_))
    }

    /// The ticket already knows its outcome (a slot or a refusal).
    pub fn refusal(&self) -> Option<WorkRefusal> {
        match self.state {
            TicketState::Resolved(Err(refusal)) => Some(refusal),
            _ => None,
        }
    }

    pub fn lane(&self) -> WorkLane {
        self.lane
    }

    /// The outcome of a ticket that resolved on arrival, without awaiting
    /// it; `None` (and the ticket back) while it is queued.
    pub fn ready(mut self) -> Result<Result<WorkPermit, WorkRefusal>, WorkTicket> {
        match std::mem::replace(&mut self.state, TicketState::Done) {
            TicketState::Resolved(Ok(())) => {
                let permit = self.permit();
                if self.is_cancelled() {
                    drop(permit);
                    return Ok(Err(WorkRefusal::Cancelled));
                }
                Ok(Ok(permit))
            }
            TicketState::Resolved(Err(refusal)) => Ok(Err(refusal)),
            TicketState::Done => Ok(Err(WorkRefusal::Cancelled)),
            queued @ TicketState::Queued(_) => {
                self.state = queued;
                Err(self)
            }
        }
    }

    fn permit(&self) -> WorkPermit {
        WorkPermit {
            gate: self.gate.clone(),
            lane: self.lane,
        }
    }

    fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled() || self.scope.as_ref().is_some_and(CancelToken::is_cancelled)
    }
}

impl Future for WorkTicket {
    type Output = Result<WorkPermit, WorkRefusal>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = &mut *self;

        match std::mem::replace(&mut this.state, TicketState::Done) {
            TicketState::Done => Poll::Ready(Err(WorkRefusal::Cancelled)),
            TicketState::Resolved(Ok(())) => {
                let permit = this.permit();
                if this.is_cancelled() {
                    drop(permit);
                    return Poll::Ready(Err(WorkRefusal::Cancelled));
                }
                Poll::Ready(Ok(permit))
            }
            TicketState::Resolved(Err(refusal)) => Poll::Ready(Err(refusal)),
            TicketState::Queued(id) => {
                let cancelled = this.is_cancelled();
                let mut wake = Vec::new();
                let resolved = {
                    let mut state = this.gate.lock();
                    let limit = state.limits.running_limit(this.lane);
                    let Some(lane_state) = state.lanes.get_mut(&this.lane) else {
                        return Poll::Ready(Err(WorkRefusal::Cancelled));
                    };
                    let outcome = lane_state
                        .waiters
                        .get(&id)
                        .map_or(Outcome::Refused(WorkRefusal::Cancelled), |w| w.outcome);

                    match outcome {
                        Outcome::Waiting if cancelled => {
                            lane_state.unqueue(id);
                            lane_state.waiters.remove(&id);
                            Some(Err(WorkRefusal::Cancelled))
                        }
                        Outcome::Waiting => {
                            if let Some(waiter) = lane_state.waiters.get_mut(&id) {
                                waiter.waker = Some(cx.waker().clone());
                            }
                            None
                        }
                        Outcome::Granted => {
                            lane_state.waiters.remove(&id);
                            if cancelled {
                                lane_state.running = lane_state.running.saturating_sub(1);
                                lane_state.promote(limit, &mut wake);
                                Some(Err(WorkRefusal::Cancelled))
                            } else {
                                Some(Ok(()))
                            }
                        }
                        Outcome::Refused(refusal) => {
                            lane_state.waiters.remove(&id);
                            Some(Err(refusal))
                        }
                    }
                };
                wake_all(wake);

                match resolved {
                    Some(Ok(())) => Poll::Ready(Ok(this.permit())),
                    Some(Err(refusal)) => Poll::Ready(Err(refusal)),
                    None => {
                        this.state = TicketState::Queued(id);
                        this.cancel.wake_on_cancel(this.listener, cx.waker());
                        if let Some(scope) = &this.scope {
                            scope.wake_on_cancel(this.listener, cx.waker());
                        }
                        Poll::Pending
                    }
                }
            }
        }
    }
}

impl Drop for WorkTicket {
    fn drop(&mut self) {
        self.cancel.forget_waker(self.listener);
        if let Some(scope) = &self.scope {
            scope.forget_waker(self.listener);
        }

        match std::mem::replace(&mut self.state, TicketState::Done) {
            TicketState::Resolved(Ok(())) => self.gate.release(self.lane),
            TicketState::Queued(id) => {
                let granted = {
                    let mut state = self.gate.lock();
                    let Some(lane_state) = state.lanes.get_mut(&self.lane) else {
                        return;
                    };
                    lane_state.unqueue(id);
                    lane_state
                        .waiters
                        .remove(&id)
                        .is_some_and(|waiter| waiter.outcome == Outcome::Granted)
                };

                if granted {
                    self.gate.release(self.lane);
                }
            }
            TicketState::Resolved(Err(_)) | TicketState::Done => {}
        }
    }
}

/// A held slot; dropping it frees the slot for the next queued work.
pub struct WorkPermit {
    gate: WorkGate,
    lane: WorkLane,
}

impl WorkPermit {
    pub fn lane(&self) -> WorkLane {
        self.lane
    }
}

impl Drop for WorkPermit {
    fn drop(&mut self) {
        self.gate.release(self.lane);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn limits(running: usize, queued: usize) -> WorkLimits {
        WorkLimits {
            max_concurrent_queries: running,
            max_queued_queries: queued,
            max_connection_tasks: running,
        }
    }

    fn request(lane: WorkLane, priority: WorkPriority) -> (WorkRequest, CancelToken) {
        let token = CancelToken::new();
        (WorkRequest::new(lane, priority, token.clone()), token)
    }

    fn poll_now(ticket: &mut WorkTicket) -> Poll<Result<WorkPermit, WorkRefusal>> {
        let waker = futures::task::noop_waker();
        let mut cx = Context::from_waker(&waker);
        Pin::new(ticket).poll(&mut cx)
    }

    #[test]
    fn at_most_the_limit_runs_at_once_and_every_unit_completes() {
        let gate = WorkGate::new(limits(2, 50));
        let lane = WorkLane::Queries(Uuid::new_v4());
        let running = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));
        let completed = Arc::new(AtomicUsize::new(0));

        let workers: Vec<_> = (0..5)
            .map(|_| {
                let (request, _token) = request(lane, WorkPriority::Background);
                let ticket = gate.admit(request);
                let running = running.clone();
                let peak = peak.clone();
                let completed = completed.clone();

                std::thread::spawn(move || {
                    let permit = futures::executor::block_on(ticket).expect("a slot");
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    peak.fetch_max(now, Ordering::SeqCst);
                    std::thread::sleep(Duration::from_millis(30));
                    running.fetch_sub(1, Ordering::SeqCst);
                    drop(permit);
                    completed.fetch_add(1, Ordering::SeqCst);
                })
            })
            .collect();

        for worker in workers {
            worker.join().expect("worker finished");
        }

        assert_eq!(completed.load(Ordering::SeqCst), 5);
        assert!(
            peak.load(Ordering::SeqCst) <= 2,
            "never more than the limit"
        );
        assert_eq!(gate.running_count(lane), 0);
        assert_eq!(gate.waiting_count(lane), 0);
    }

    #[test]
    fn a_queued_unit_cancelled_by_its_token_never_runs() {
        let gate = WorkGate::new(limits(1, 50));
        let lane = WorkLane::Queries(Uuid::new_v4());

        let (first, _t1) = request(lane, WorkPriority::Background);
        let mut first = gate.admit(first);
        let first_permit = match poll_now(&mut first) {
            Poll::Ready(Ok(permit)) => permit,
            _ => panic!("the first unit runs at once"),
        };

        let (second, second_token) = request(lane, WorkPriority::Background);
        let mut second = gate.admit(second);
        assert!(second.is_queued());
        assert!(poll_now(&mut second).is_pending());
        assert_eq!(gate.waiting_count(lane), 1);

        second_token.cancel();

        assert_eq!(gate.waiting_count(lane), 0);
        assert!(matches!(
            poll_now(&mut second),
            Poll::Ready(Err(WorkRefusal::Cancelled))
        ));

        drop(first_permit);
        assert_eq!(
            gate.running_count(lane),
            0,
            "the cancelled unit took no slot"
        );
    }

    #[test]
    fn interactive_work_starts_before_earlier_queued_background_work() {
        let gate = WorkGate::new(limits(1, 50));
        let lane = WorkLane::Queries(Uuid::new_v4());

        let (holder, _t) = request(lane, WorkPriority::Background);
        let mut holder = gate.admit(holder);
        let holder_permit = match poll_now(&mut holder) {
            Poll::Ready(Ok(permit)) => permit,
            _ => panic!("the first unit runs at once"),
        };

        let (background, _tb) = request(lane, WorkPriority::Background);
        let mut background = gate.admit(background);
        let (interactive, _ti) = request(lane, WorkPriority::Interactive);
        let mut interactive = gate.admit(interactive);
        assert!(poll_now(&mut background).is_pending());
        assert!(poll_now(&mut interactive).is_pending());

        drop(holder_permit);

        let Poll::Ready(Ok(_interactive_permit)) = poll_now(&mut interactive) else {
            panic!("interactive work takes the freed slot");
        };
        assert!(poll_now(&mut background).is_pending());
    }

    #[test]
    fn a_full_queue_refuses_background_work() {
        let gate = WorkGate::new(limits(1, 2));
        let lane = WorkLane::Queries(Uuid::new_v4());

        let tickets: Vec<WorkTicket> = (0..3)
            .map(|_| gate.admit(request(lane, WorkPriority::Background).0))
            .collect();
        assert!(!tickets[0].is_queued());
        assert!(tickets[1].is_queued() && tickets[2].is_queued());

        let fourth = gate.admit(request(lane, WorkPriority::Background).0);

        assert_eq!(
            fourth.refusal(),
            Some(WorkRefusal::QueueFull { waiting: 2 })
        );
        assert_eq!(gate.waiting_count(lane), 2);
    }

    #[test]
    fn interactive_work_on_a_full_queue_evicts_the_oldest_background_work() {
        let gate = WorkGate::new(limits(1, 2));
        let lane = WorkLane::Queries(Uuid::new_v4());

        let _running = gate.admit(request(lane, WorkPriority::Background).0);
        let mut oldest = gate.admit(request(lane, WorkPriority::Background).0);
        let mut newer = gate.admit(request(lane, WorkPriority::Background).0);
        assert!(poll_now(&mut oldest).is_pending());

        let mut interactive = gate.admit(request(lane, WorkPriority::Interactive).0);

        assert!(interactive.is_queued());
        assert!(matches!(
            poll_now(&mut oldest),
            Poll::Ready(Err(WorkRefusal::QueueFull { waiting: 2 }))
        ));
        assert!(poll_now(&mut newer).is_pending());
        assert!(poll_now(&mut interactive).is_pending());
        assert_eq!(gate.waiting_count(lane), 2);
    }

    #[test]
    fn interactive_work_is_refused_when_only_interactive_work_waits() {
        let gate = WorkGate::new(limits(1, 1));
        let lane = WorkLane::Queries(Uuid::new_v4());

        let _running = gate.admit(request(lane, WorkPriority::Background).0);
        let _queued = gate.admit(request(lane, WorkPriority::Interactive).0);
        let refused = gate.admit(request(lane, WorkPriority::Interactive).0);

        assert_eq!(
            refused.refusal(),
            Some(WorkRefusal::QueueFull { waiting: 1 })
        );
    }

    #[test]
    fn limits_are_per_connection() {
        let gate = WorkGate::new(limits(1, 50));
        let busy = WorkLane::Queries(Uuid::new_v4());
        let other = WorkLane::Queries(Uuid::new_v4());

        let _running = gate.admit(request(busy, WorkPriority::Background).0);
        let queued = gate.admit(request(busy, WorkPriority::Background).0);
        assert!(queued.is_queued());

        let elsewhere = gate.admit(request(other, WorkPriority::Background).0);

        assert!(!elsewhere.is_queued() && elsewhere.refusal().is_none());
        assert_eq!(gate.running_count(other), 1);
    }

    #[test]
    fn the_connection_task_lane_queues_instead_of_refusing() {
        let gate = WorkGate::new(limits(1, 1));
        let lane = WorkLane::ConnectionTasks;

        let mut first = gate.admit(request(lane, WorkPriority::Background).0);
        let permit = match poll_now(&mut first) {
            Poll::Ready(Ok(permit)) => permit,
            _ => panic!("a free slot"),
        };
        let mut queued: Vec<WorkTicket> = (0..3)
            .map(|_| gate.admit(request(lane, WorkPriority::Background).0))
            .collect();

        assert!(queued.iter().all(WorkTicket::is_queued), "no queue cap");

        drop(permit);

        let Poll::Ready(Ok(_next_permit)) = poll_now(&mut queued[0]) else {
            panic!("the oldest queued task takes the freed slot");
        };
        assert!(poll_now(&mut queued[1]).is_pending());
    }

    #[test]
    fn clear_queue_cancels_waiting_work_and_keeps_running_work() {
        let gate = WorkGate::new(limits(1, 50));
        let lane = WorkLane::Queries(Uuid::new_v4());
        let mut running = gate.admit(request(lane, WorkPriority::Background).0);
        let running_permit = match poll_now(&mut running) {
            Poll::Ready(Ok(permit)) => permit,
            _ => panic!("a free slot"),
        };
        let task_ids: Vec<TaskId> = (0..4).map(|_| TaskId::new_v4()).collect();
        let mut queued: Vec<WorkTicket> = task_ids
            .iter()
            .map(|task_id| {
                gate.admit(
                    request(lane, WorkPriority::Background)
                        .0
                        .with_task(*task_id),
                )
            })
            .collect();

        let cleared = gate.clear_queue(lane);

        assert_eq!(cleared, task_ids);
        for ticket in &mut queued {
            assert!(matches!(
                poll_now(ticket),
                Poll::Ready(Err(WorkRefusal::Cancelled))
            ));
        }
        assert_eq!(gate.running_count(lane), 1);
        drop(running_permit);
        assert_eq!(gate.running_count(lane), 0);
    }

    #[test]
    fn a_dropped_scope_drops_its_queued_work() {
        let gate = WorkGate::new(limits(1, 50));
        let lane = WorkLane::Queries(Uuid::new_v4());
        let _running = gate.admit(request(lane, WorkPriority::Background).0);
        let scope = CancelToken::new();
        let mut queued = gate.admit(
            request(lane, WorkPriority::Background)
                .0
                .with_scope(Some(scope.clone())),
        );
        assert!(poll_now(&mut queued).is_pending());

        scope.cancel();

        assert!(matches!(
            poll_now(&mut queued),
            Poll::Ready(Err(WorkRefusal::Cancelled))
        ));
    }

    #[test]
    fn a_dropped_ticket_leaves_the_queue_and_hands_a_granted_slot_on() {
        let gate = WorkGate::new(limits(1, 50));
        let lane = WorkLane::Queries(Uuid::new_v4());
        let mut first = gate.admit(request(lane, WorkPriority::Background).0);
        let permit = match poll_now(&mut first) {
            Poll::Ready(Ok(permit)) => permit,
            _ => panic!("a free slot"),
        };
        let granted_but_unpolled = gate.admit(request(lane, WorkPriority::Background).0);
        let mut last = gate.admit(request(lane, WorkPriority::Background).0);

        drop(permit);
        assert_eq!(gate.running_count(lane), 1, "the second unit got the slot");
        drop(granted_but_unpolled);

        assert!(matches!(poll_now(&mut last), Poll::Ready(Ok(_))));
    }

    #[test]
    fn a_task_bound_permit_frees_its_slot_when_the_task_finishes() {
        use super::super::task::{TaskKind, TaskManager};

        let gate = WorkGate::new(limits(1, 50));
        let lane = WorkLane::ConnectionTasks;
        let mut tasks = TaskManager::new();
        let (task_id, _token) = tasks.start(TaskKind::Connect, "Connect");
        let mut ticket = gate.admit(request(lane, WorkPriority::Background).0);
        let Poll::Ready(Ok(permit)) = poll_now(&mut ticket) else {
            panic!("a free slot");
        };
        tasks.attach_permit(task_id, permit);
        let mut next = gate.admit(request(lane, WorkPriority::Background).0);
        assert!(poll_now(&mut next).is_pending());

        tasks.complete(task_id);

        assert!(matches!(poll_now(&mut next), Poll::Ready(Ok(_))));
    }

    #[test]
    fn a_raised_limit_starts_queued_work() {
        let gate = WorkGate::new(limits(1, 50));
        let lane = WorkLane::Queries(Uuid::new_v4());
        let _running = gate.admit(request(lane, WorkPriority::Background).0);
        let mut queued = gate.admit(request(lane, WorkPriority::Background).0);
        assert!(poll_now(&mut queued).is_pending());

        gate.set_limits(limits(2, 50));

        assert!(matches!(poll_now(&mut queued), Poll::Ready(Ok(_))));
    }
}

//! Runs database work through the app's [`dbflux_core::WorkGate`], so a
//! connection never gets more than its configured number of queries at once
//! and the rest wait, visibly, in the Tasks panel.
//!
//! [`spawn_query`] replaces `cx.background_executor().spawn(..)` for the
//! future of one blocking call against a connection. The slot it waits for is moved into
//! that call and freed when the call returns, before the caller's
//! continuation runs, so follow-up work admitted from the continuation never
//! waits on a slot its own caller still holds (the gate's nesting rule).
//!
//! Connection tasks (connects, schema loads) use [`admit_connection_task`]:
//! their slot is tied to the task and frees when the task finishes.

use dbflux_core::{
    CancelToken, DbError, TaskId, TaskKind, TaskTarget, WorkLane, WorkPermit, WorkPriority,
    WorkRefusal, WorkRequest, WorkTicket,
};
use gpui::{App, AsyncApp, Entity, Task};
use std::future::Future;
use uuid::Uuid;

use crate::app_state_entity::{AppStateChanged, AppStateEntity};

/// What a unit of query work is and who waits for it.
pub struct QueryWork {
    profile_id: Uuid,
    database: Option<String>,
    priority: WorkPriority,
    task: Option<(TaskId, Option<CancelToken>)>,
    description: Option<String>,
    scope: Option<CancelToken>,
}

impl QueryWork {
    /// Work nobody waits for in front of the screen: page loads, counts,
    /// lookups, dashboards.
    pub fn background(profile_id: Uuid) -> Self {
        Self::new(profile_id, WorkPriority::Background)
    }

    /// Work the user just asked for and waits on: an explicit Run, a Save.
    pub fn interactive(profile_id: Uuid) -> Self {
        Self::new(profile_id, WorkPriority::Interactive)
    }

    fn new(profile_id: Uuid, priority: WorkPriority) -> Self {
        Self {
            profile_id,
            database: None,
            priority,
            task: None,
            description: None,
            scope: None,
        }
    }

    /// The task that reports this work; cancelling it while the work waits
    /// takes the work out of the queue.
    pub fn for_task(mut self, task_id: TaskId, cancel_token: &CancelToken) -> Self {
        self.task = Some((task_id, Some(cancel_token.clone())));
        self
    }

    /// [`Self::for_task`] for callers that hold only the task id; the token
    /// is looked up when the work is admitted.
    pub fn for_task_id(mut self, task_id: TaskId) -> Self {
        self.task = Some((task_id, None));
        self
    }

    /// The Tasks panel label of work without a task of its own, shown only
    /// while it waits.
    pub fn described(mut self, description: impl Into<String>) -> Self {
        self.description = Some(description.into());
        self
    }

    pub fn on_database(mut self, database: Option<String>) -> Self {
        self.database = database;
        self
    }

    /// The lifetime of the tab that asked for the work: once it is
    /// cancelled, work still waiting is dropped.
    pub fn scoped(mut self, scope: &CancelToken) -> Self {
        self.scope = Some(scope.clone());
        self
    }
}

/// Runs `job` on the background executor once the connection has a free
/// query slot. Resolves to the refusal when the work was cancelled while it
/// waited or the queue was full; the task (if any) is then already marked
/// cancelled and a full queue has shown its notice.
pub fn spawn_query<R: Send + 'static>(
    app_state: &Entity<AppStateEntity>,
    work: QueryWork,
    job: impl Future<Output = R> + Send + 'static,
    cx: &mut App,
) -> Task<Result<R, WorkRefusal>> {
    spawn_query_with(app_state, work, async move { Ok(job.await) }, Err, cx)
}

/// [`spawn_query`] for a driver call: a refusal reads as
/// [`DbError::Cancelled`], which callers already treat as a cancelled run.
#[expect(
    clippy::result_large_err,
    reason = "the refusal must be the driver call's own `Result<T, DbError>`, so callers keep their error handling"
)]
pub fn spawn_query_db<T: Send + 'static>(
    app_state: &Entity<AppStateEntity>,
    work: QueryWork,
    job: impl Future<Output = Result<T, DbError>> + Send + 'static,
    cx: &mut App,
) -> Task<Result<T, DbError>> {
    spawn_query_with(app_state, work, job, |_| Err(DbError::Cancelled), cx)
}

/// Runs `job` once the connection has a free query slot, or resolves to
/// `refused(..)` when the work never gets one. With a slot free on arrival
/// the job is spawned on the background executor straight away, so work
/// below the limit is scheduled exactly as without a queue.
pub fn spawn_query_with<R: Send + 'static>(
    app_state: &Entity<AppStateEntity>,
    work: QueryWork,
    job: impl Future<Output = R> + Send + 'static,
    refused: impl FnOnce(WorkRefusal) -> R + 'static,
    cx: &mut App,
) -> Task<R> {
    let admitted = admit_query(app_state, work, cx);
    let executor = cx.background_executor().clone();

    let admitted = match admitted.try_ready() {
        Ok(Ok(permit)) => {
            return executor.spawn(async move {
                let _permit = permit;
                job.await
            });
        }
        Ok(Err(refusal)) => return Task::ready(refused(refusal)),
        Err(queued) => queued,
    };

    let app_state = app_state.clone();
    cx.spawn(async move |cx| match admitted.wait(&app_state, cx).await {
        Ok(permit) => {
            executor
                .spawn(async move {
                    let _permit = permit;
                    job.await
                })
                .await
        }
        Err(refusal) => refused(refusal),
    })
}

/// [`spawn_query_db`] from inside a `cx.spawn` future.
pub fn spawn_query_db_async<T: Send + 'static>(
    app_state: &Entity<AppStateEntity>,
    work: QueryWork,
    job: impl Future<Output = Result<T, DbError>> + Send + 'static,
    cx: &mut AsyncApp,
) -> Task<Result<T, DbError>> {
    cx.update(|cx| spawn_query_db(app_state, work, job, cx))
}

/// A ticket together with the task bookkeeping that follows it.
pub struct AdmittedWork {
    ticket: WorkTicket,
    task_id: Option<TaskId>,
    /// A task made only to show waiting work that has no task of its own;
    /// removed again when the work gets its slot.
    placeholder: bool,
    connection_name: String,
}

impl AdmittedWork {
    pub fn is_queued(&self) -> bool {
        self.ticket.is_queued()
    }

    /// The slot, or the refusal, of work that did not have to queue; the
    /// queued work back otherwise. A refusal on arrival was settled already.
    pub fn try_ready(self) -> Result<Result<WorkPermit, WorkRefusal>, AdmittedWork> {
        if self.ticket.is_queued() {
            return Err(self);
        }

        let AdmittedWork { ticket, .. } = self;
        match ticket.ready() {
            Ok(outcome) => Ok(outcome),
            Err(_) => Ok(Err(WorkRefusal::Cancelled)),
        }
    }

    /// Waits for the slot and settles the task: back to running when it
    /// waited, cancelled when the work was refused.
    pub async fn wait(
        self,
        app_state: &Entity<AppStateEntity>,
        cx: &mut AsyncApp,
    ) -> Result<WorkPermit, WorkRefusal> {
        let AdmittedWork {
            ticket,
            task_id,
            placeholder,
            connection_name,
        } = self;
        let was_queued = ticket.is_queued();
        let lane = ticket.lane();
        let result = ticket.await;

        let refusal = result.as_ref().err().copied();
        if !was_queued && refusal.is_none() {
            return result;
        }

        cx.update(|cx| {
            settle(
                app_state,
                lane,
                task_id,
                placeholder,
                refusal,
                &connection_name,
                cx,
            );
        });

        result
    }
}

/// Admits query work and marks it waiting when it has to queue. A refusal on
/// arrival (a full queue) is settled here, so the returned work resolves to
/// it straight away.
pub fn admit_query(
    app_state: &Entity<AppStateEntity>,
    work: QueryWork,
    cx: &mut App,
) -> AdmittedWork {
    let QueryWork {
        profile_id,
        database,
        priority,
        task,
        description,
        scope,
    } = work;
    let lane = WorkLane::Queries(profile_id);
    let (task_id, cancel) = match task {
        Some((task_id, Some(token))) => (Some(task_id), token),
        Some((task_id, None)) => (
            Some(task_id),
            app_state
                .read(cx)
                .tasks()
                .cancel_token(task_id)
                .unwrap_or_default(),
        ),
        None => (None, CancelToken::new()),
    };

    let mut request = WorkRequest::new(lane, priority, cancel.clone()).with_scope(scope);
    if let Some(task_id) = task_id {
        request = request.with_task(task_id);
    }

    let (admitted, refusal) = app_state.update(cx, |state, cx| {
        let connection_name = connection_name(state, profile_id);
        let ticket = state.work_gate().admit(request);
        let mut admitted = AdmittedWork {
            ticket,
            task_id,
            placeholder: false,
            connection_name,
        };

        if admitted.ticket.is_queued() {
            let reason = waiting_reason(&admitted.connection_name);

            let task_id = match admitted.task_id {
                Some(task_id) => task_id,
                None => {
                    let target = TaskTarget {
                        profile_id,
                        database,
                    };
                    let task_id = state.tasks_mut().start_with_token(
                        TaskKind::Query,
                        description.unwrap_or_else(|| {
                            dbflux_i18n::t!("work_queue.untitled_query").to_string()
                        }),
                        Some(target),
                        cancel,
                    );
                    admitted.task_id = Some(task_id);
                    admitted.placeholder = true;
                    state.work_gate().attach_task(&admitted.ticket, task_id);
                    task_id
                }
            };

            state.tasks_mut().set_waiting(task_id, Some(reason));
            cx.emit(AppStateChanged);
        }

        let refusal = admitted.ticket.refusal();
        (admitted, refusal)
    });

    if let Some(refusal) = refusal {
        settle(
            app_state,
            lane,
            admitted.task_id,
            false,
            Some(refusal),
            &admitted.connection_name,
            cx,
        );
    }

    admitted
}

/// Queues a connection task behind the connection-task limit. The task is
/// marked waiting while it queues; once it has a slot, the slot belongs to
/// the task and frees on its terminal transition. Resolves to `false` when
/// the task was cancelled while it waited.
pub fn admit_connection_task(
    app_state: &Entity<AppStateEntity>,
    task_id: TaskId,
    cancel_token: &CancelToken,
    cx: &mut App,
) -> Task<bool> {
    let request = WorkRequest::new(
        WorkLane::ConnectionTasks,
        WorkPriority::Background,
        cancel_token.clone(),
    )
    .with_task(task_id);

    // A free slot is tied to the task right here, so work below the limit
    // starts without an extra trip through the executor.
    let queued = app_state.update(cx, |state, cx| {
        let ticket = state.work_gate().admit(request);
        let ticket = match ticket.ready() {
            Ok(Ok(permit)) => {
                state.tasks_mut().attach_permit(task_id, permit);
                return None;
            }
            Ok(Err(_)) => {
                state.tasks_mut().cancel(task_id);
                cx.emit(AppStateChanged);
                return Some(Err(()));
            }
            Err(queued) => queued,
        };
        state.tasks_mut().set_waiting(
            task_id,
            Some(dbflux_i18n::t!("work_queue.waiting_for_connection_tasks").to_string()),
        );
        cx.emit(AppStateChanged);
        Some(Ok(ticket))
    });

    let ticket = match queued {
        None => return Task::ready(true),
        Some(Err(())) => return Task::ready(false),
        Some(Ok(ticket)) => ticket,
    };

    let app_state = app_state.clone();
    cx.spawn(async move |cx| {
        let result = ticket.await;

        cx.update(|cx| {
            app_state.update(cx, |state, cx| match result {
                Ok(permit) => {
                    state.tasks_mut().set_waiting(task_id, None);
                    state.tasks_mut().attach_permit(task_id, permit);
                    cx.emit(AppStateChanged);
                    true
                }
                Err(_) => {
                    state.tasks_mut().cancel(task_id);
                    cx.emit(AppStateChanged);
                    false
                }
            })
        })
    })
}

/// A connection-task slot for work that has no task of its own yet.
pub enum ConnectionTaskSlot {
    /// A slot was free; tie it to the task the work starts.
    Free(WorkPermit),
    /// The work waits, shown as a waiting task; resolves to the slot, or to
    /// `None` when the wait was cancelled.
    Queued(Task<Option<WorkPermit>>),
}

/// Takes a connection-task slot for work that would start its task only
/// once it can run (it dispatches synchronously when it starts). While it
/// waits, a task with `kind` and `description` stands in for it in the
/// Tasks panel, where it can be cancelled.
pub fn connection_task_slot(
    app_state: &Entity<AppStateEntity>,
    kind: TaskKind,
    description: String,
    target: Option<TaskTarget>,
    cx: &mut App,
) -> ConnectionTaskSlot {
    let cancel = CancelToken::new();
    let request = WorkRequest::new(
        WorkLane::ConnectionTasks,
        WorkPriority::Background,
        cancel.clone(),
    );

    let (ticket, placeholder) = app_state.update(cx, |state, cx| {
        let ticket = state.work_gate().admit(request);
        if !ticket.is_queued() {
            return (ticket, None);
        }

        let task_id = state
            .tasks_mut()
            .start_with_token(kind, description, target, cancel);
        state.work_gate().attach_task(&ticket, task_id);
        state.tasks_mut().set_waiting(
            task_id,
            Some(dbflux_i18n::t!("work_queue.waiting_for_connection_tasks").to_string()),
        );
        cx.emit(AppStateChanged);
        (ticket, Some(task_id))
    });

    let Some(placeholder) = placeholder else {
        return match ticket.ready() {
            Ok(Ok(permit)) => ConnectionTaskSlot::Free(permit),
            _ => ConnectionTaskSlot::Queued(Task::ready(None)),
        };
    };

    let app_state = app_state.clone();
    ConnectionTaskSlot::Queued(cx.spawn(async move |cx| {
        let result = ticket.await;

        cx.update(|cx| {
            app_state.update(cx, |state, cx| {
                match &result {
                    Ok(_) => state.tasks_mut().remove(placeholder),
                    Err(_) => {
                        state.tasks_mut().cancel(placeholder);
                    }
                }
                cx.emit(AppStateChanged);
            })
        });

        result.ok()
    }))
}

fn settle(
    app_state: &Entity<AppStateEntity>,
    lane: WorkLane,
    task_id: Option<TaskId>,
    placeholder: bool,
    refusal: Option<WorkRefusal>,
    connection_name: &str,
    cx: &mut App,
) {
    app_state.update(cx, |state, cx| {
        if let Some(task_id) = task_id {
            match refusal {
                None if placeholder => state.tasks_mut().remove(task_id),
                None => state.tasks_mut().set_waiting(task_id, None),
                Some(_) => {
                    state.tasks_mut().cancel(task_id);
                }
            }
        }
        cx.emit(AppStateChanged);
    });

    if let Some(WorkRefusal::QueueFull { waiting }) = refusal {
        let message = dbflux_i18n::t!(
            "work_queue.queue_full",
            connection = connection_name,
            count = waiting
        )
        .to_string();
        log::info!("{message} ({lane:?})");

        if cx.has_global::<crate::toast::ToastGlobal>() {
            crate::toast::Toast::warning(message).push(cx);
        }
    }
}

fn connection_name(state: &AppStateEntity, profile_id: Uuid) -> String {
    state
        .connections()
        .get(&profile_id)
        .map(|connected| connected.profile.name.clone())
        .or_else(|| {
            state
                .profiles()
                .iter()
                .find(|profile| profile.id == profile_id)
                .map(|profile| profile.name.clone())
        })
        .unwrap_or_else(|| profile_id.to_string())
}

fn waiting_reason(connection_name: &str) -> String {
    dbflux_i18n::t!("work_queue.waiting_for_slot", connection = connection_name).to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{AppContext as _, TestAppContext};

    fn app_state_with_connection_task_limit(
        cx: &mut TestAppContext,
        limit: usize,
    ) -> Entity<AppStateEntity> {
        let app_state = cx.update(|cx| {
            cx.new(|_| {
                let runtime = dbflux_storage::bootstrap::StorageRuntime::in_memory()
                    .expect("in-memory storage");
                AppStateEntity::new_with_storage_runtime(runtime).expect("test storage setup")
            })
        });
        cx.update(|cx| {
            app_state.update(cx, |state, _| {
                let mut settings = state.general_settings().clone();
                settings.max_concurrent_background_tasks = limit;
                state.update_general_settings(settings);
            });
        });
        app_state
    }

    fn start_connect(
        app_state: &Entity<AppStateEntity>,
        cx: &mut TestAppContext,
    ) -> (TaskId, Task<bool>) {
        cx.update(|cx| {
            let (task_id, token) = app_state.update(cx, |state, _| {
                state.start_task(TaskKind::Connect, "Connecting")
            });
            let admission = admit_connection_task(app_state, task_id, &token, cx);
            (task_id, admission)
        })
    }

    fn waiting_for(
        app_state: &Entity<AppStateEntity>,
        task_id: TaskId,
        cx: &mut TestAppContext,
    ) -> Option<String> {
        cx.read(|cx| {
            app_state
                .read(cx)
                .tasks()
                .get(task_id)
                .and_then(|task| task.waiting_for)
        })
    }

    #[gpui::test]
    fn a_connect_past_the_limit_queues_and_starts_when_a_slot_frees(cx: &mut TestAppContext) {
        let app_state = app_state_with_connection_task_limit(cx, 1);
        let (first, first_admission) = start_connect(&app_state, cx);
        cx.run_until_parked();
        assert!(waiting_for(&app_state, first, cx).is_none(), "a free slot");

        let (second, second_admission) = start_connect(&app_state, cx);
        cx.run_until_parked();

        let status = cx.read(|cx| app_state.read(cx).tasks().get(second).map(|t| t.status));
        assert_eq!(
            status,
            Some(dbflux_core::TaskStatus::Running),
            "past the limit the connect waits instead of being refused"
        );
        assert!(waiting_for(&app_state, second, cx).is_some());

        cx.update(|cx| app_state.update(cx, |state, _| state.complete_task(first)));
        cx.run_until_parked();

        assert!(waiting_for(&app_state, second, cx).is_none());
        let running = cx.read(|cx| {
            app_state
                .read(cx)
                .work_gate()
                .running_count(WorkLane::ConnectionTasks)
        });
        assert_eq!(running, 1, "the queued connect took the freed slot");
        drop((first_admission, second_admission));
    }

    #[gpui::test]
    fn a_queued_connect_cancelled_while_it_waits_never_starts(cx: &mut TestAppContext) {
        let app_state = app_state_with_connection_task_limit(cx, 1);
        let (_first, _first_admission) = start_connect(&app_state, cx);
        let (second, second_admission) = start_connect(&app_state, cx);
        cx.run_until_parked();

        cx.update(|cx| {
            app_state.update(cx, |state, _| {
                state.tasks_mut().cancel(second);
            })
        });
        cx.run_until_parked();
        drop(second_admission);

        let waiting = cx.read(|cx| {
            app_state
                .read(cx)
                .work_gate()
                .waiting_count(WorkLane::ConnectionTasks)
        });
        assert_eq!(waiting, 0, "the cancelled connect left the queue");
        let status = cx.read(|cx| app_state.read(cx).tasks().get(second).map(|t| t.status));
        assert_eq!(status, Some(dbflux_core::TaskStatus::Cancelled));
    }
}

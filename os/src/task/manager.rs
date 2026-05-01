use super::task::BIG_STRIDE;
use super::{ProcessControlBlock, TaskControlBlock, TaskStatus};
use crate::sync::UPIntrFreeCell;
use alloc::collections::{BTreeMap, VecDeque};
use alloc::sync::Arc;
use lazy_static::*;

/// Stride scheduler.
///
/// Replaces the original FIFO scheduler. Each pick selects the runnable task
/// with the smallest accumulated `stride`. After being picked, the task's
/// stride is incremented by `BIG_STRIDE / priority`, so high-priority tasks
/// (large priority) accumulate stride more slowly and get scheduled more
/// often. This gives proportional-share fairness while preserving O(1) `add`
/// and O(n) `fetch` (n = ready queue length, typically small).
pub struct TaskManager {
    ready_queue: VecDeque<Arc<TaskControlBlock>>,
}

impl TaskManager {
    pub fn new() -> Self {
        Self {
            ready_queue: VecDeque::new(),
        }
    }
    pub fn add(&mut self, task: Arc<TaskControlBlock>) {
        self.ready_queue.push_back(task);
    }
    pub fn fetch(&mut self) -> Option<Arc<TaskControlBlock>> {
        if self.ready_queue.is_empty() {
            return None;
        }
        // Linear scan for the runnable task with the smallest stride.
        let mut min_idx = 0usize;
        let mut min_stride = u64::MAX;
        for (i, task) in self.ready_queue.iter().enumerate() {
            let s = task.inner_exclusive_access().stride;
            if s < min_stride {
                min_stride = s;
                min_idx = i;
            }
        }
        let task = self.ready_queue.remove(min_idx).unwrap();
        // Advance the picked task's stride by BIG_STRIDE / priority. The
        // priority floor of 2 prevents division blow-up if it is ever
        // mis-set; sys_set_priority enforces the same lower bound.
        let mut inner = task.inner_exclusive_access();
        let prio = inner.priority.max(2);
        inner.stride = inner.stride.wrapping_add(BIG_STRIDE / prio);
        drop(inner);
        Some(task)
    }
}

lazy_static! {
    pub static ref TASK_MANAGER: UPIntrFreeCell<TaskManager> =
        unsafe { UPIntrFreeCell::new(TaskManager::new()) };
    pub static ref PID2PCB: UPIntrFreeCell<BTreeMap<usize, Arc<ProcessControlBlock>>> =
        unsafe { UPIntrFreeCell::new(BTreeMap::new()) };
}

pub fn add_task(task: Arc<TaskControlBlock>) {
    TASK_MANAGER.exclusive_access().add(task);
}

pub fn wakeup_task(task: Arc<TaskControlBlock>) {
    let mut task_inner = task.inner_exclusive_access();
    task_inner.task_status = TaskStatus::Ready;
    drop(task_inner);
    add_task(task);
}

pub fn fetch_task() -> Option<Arc<TaskControlBlock>> {
    TASK_MANAGER.exclusive_access().fetch()
}

pub fn pid2process(pid: usize) -> Option<Arc<ProcessControlBlock>> {
    let map = PID2PCB.exclusive_access();
    map.get(&pid).map(Arc::clone)
}

pub fn insert_into_pid2process(pid: usize, process: Arc<ProcessControlBlock>) {
    PID2PCB.exclusive_access().insert(pid, process);
}

pub fn remove_from_pid2process(pid: usize) {
    let mut map = PID2PCB.exclusive_access();
    if map.remove(&pid).is_none() {
        panic!("cannot find pid {} in pid2task!", pid);
    }
}

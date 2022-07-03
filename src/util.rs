use std::collections::VecDeque;

pub fn rotate_queue<T>(queue: &mut VecDeque<T>, value: T, capacity: usize) -> Option<T> {
    let popped = if queue.len() == capacity {
        queue.pop_front()
    } else {
        None
    };

    queue.push_back(value);

    popped
}

//! Port blocks: each workspace (and each agent worktree group) gets `port_block_size` ports.

use std::net::TcpListener;

use crate::Result;
use crate::store::{PortBlock, Store};

/// Allocates the lowest free block at or above `start` for `owner`, skipping blocks held by other
/// owners and ports something on the machine already listens on.
pub fn allocate(
    store: &Store,
    owner: &str,
    workspace_id: &str,
    start: u16,
    size: u16,
) -> Result<PortBlock> {
    allocate_with(store, owner, workspace_id, start, size, is_free)
}

/// [`allocate`] with the check of whether a port is free given, so that tests do not depend on what
/// listens on the machine.
fn allocate_with(
    store: &Store,
    owner: &str,
    workspace_id: &str,
    start: u16,
    size: u16,
    is_free: impl Fn(u16) -> bool,
) -> Result<PortBlock> {
    if let Some(existing) = store.port_block(owner)? {
        return Ok(existing);
    }
    let taken = store.port_blocks()?;
    let mut base = start;
    loop {
        let end = base
            .checked_add(size)
            .ok_or_else(|| crate::Error::Unavailable("no free port block left".into()))?;
        let overlaps = taken
            .iter()
            .any(|block| base < block.base + block.size && block.base < end);
        if !overlaps && (base..end).all(&is_free) {
            let block = PortBlock {
                owner: owner.to_string(),
                workspace_id: workspace_id.to_string(),
                base,
                size,
            };
            store.insert_port_block(&block)?;
            return Ok(block);
        }
        base = end;
    }
}

fn is_free(port: u16) -> bool {
    TcpListener::bind(("127.0.0.1", port)).is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_free(_: u16) -> bool {
        true
    }

    #[test]
    fn blocks_do_not_overlap_and_are_stable_per_owner() {
        let store = Store::open_in_memory().unwrap();
        let a = allocate_with(&store, "a", "a", 52000, 10, all_free).unwrap();
        assert_eq!(a.base, 52000);
        let b = allocate_with(&store, "b", "b", 52000, 10, all_free).unwrap();
        assert_eq!(b.base, 52010);
        assert_eq!(
            allocate_with(&store, "a", "a", 52000, 10, all_free)
                .unwrap()
                .base,
            52000
        );
        store.release_port_block("a").unwrap();
        assert_eq!(
            allocate_with(&store, "c", "c", 52000, 10, all_free)
                .unwrap()
                .base,
            52000
        );
    }

    #[test]
    fn a_block_with_a_busy_port_is_skipped() {
        let store = Store::open_in_memory().unwrap();
        let block = allocate_with(&store, "a", "a", 52000, 10, |port| port != 52004).unwrap();
        assert_eq!(block.base, 52010);
    }

    #[test]
    fn running_out_of_ports_is_an_error() {
        let store = Store::open_in_memory().unwrap();
        assert!(allocate_with(&store, "a", "a", 65530, 10, all_free).is_err());
        assert!(allocate_with(&store, "b", "b", 52000, 10, |_| false).is_err());
    }

    #[test]
    fn a_port_something_listens_on_is_not_free() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        assert!(!is_free(port));
    }
}

use std::collections::{HashSet, VecDeque};

use crate::plugins::PluginDefinition;

#[derive(Clone, Copy)]
pub(super) struct PluginWriteRequest {
    pub(super) plugin: &'static PluginDefinition,
    pub(super) enabled: bool,
}

#[derive(Default)]
pub(super) struct PluginWriteQueue {
    pending: HashSet<&'static str>,
    queued: VecDeque<PluginWriteRequest>,
    active: Option<&'static str>,
}

impl PluginWriteQueue {
    pub(super) fn is_pending(&self, plugin_id: &str) -> bool {
        self.pending.contains(plugin_id)
    }

    pub(super) fn enqueue(&mut self, plugin: &'static PluginDefinition, enabled: bool) -> bool {
        if !self.pending.insert(plugin.id) {
            return false;
        }
        self.queued
            .push_back(PluginWriteRequest { plugin, enabled });
        true
    }

    pub(super) fn begin_next(&mut self) -> Option<PluginWriteRequest> {
        if self.active.is_some() {
            return None;
        }
        let request = self.queued.pop_front()?;
        self.active = Some(request.plugin.id);
        Some(request)
    }

    pub(super) fn finish(&mut self, plugin_id: &'static str) {
        debug_assert_eq!(self.active, Some(plugin_id));
        self.active = None;
        self.pending.remove(plugin_id);
    }
}

#[cfg(test)]
mod tests {
    use super::PluginWriteQueue;

    #[test]
    fn distinct_plugins_wait_in_order_without_blocking_enqueue() {
        let plugins = crate::plugins::all();
        assert!(plugins.len() >= 2);
        let mut queue = PluginWriteQueue::default();

        assert!(queue.enqueue(&plugins[0], true));
        assert!(queue.enqueue(&plugins[1], false));
        assert!(queue.is_pending(plugins[0].id));
        assert!(queue.is_pending(plugins[1].id));

        let first = queue.begin_next().expect("first request should start");
        assert_eq!(first.plugin.id, plugins[0].id);
        assert!(queue.begin_next().is_none());

        queue.finish(first.plugin.id);
        let second = queue.begin_next().expect("second request should start");
        assert_eq!(second.plugin.id, plugins[1].id);
        assert!(!second.enabled);
    }

    #[test]
    fn one_plugin_cannot_enqueue_twice_while_pending() {
        let plugin = &crate::plugins::all()[0];
        let mut queue = PluginWriteQueue::default();

        assert!(queue.enqueue(plugin, true));
        assert!(!queue.enqueue(plugin, false));
        let request = queue.begin_next().expect("request should start");
        queue.finish(request.plugin.id);
        assert!(queue.enqueue(plugin, false));
    }
}

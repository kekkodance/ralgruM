use std::collections::HashSet;

use super::DeezerFeedbackKind;

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub(crate) struct DeezerFeedbackKey {
    pub(crate) kind: DeezerFeedbackKind,
    pub(crate) id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct DeezerFeedbackTicket {
    key: DeezerFeedbackKey,
    account_scope: String,
    session_generation: u64,
}

pub(crate) struct DeezerActionState {
    account_scope: String,
    session_generation: u64,
    pending_feedback: HashSet<DeezerFeedbackKey>,
}

impl DeezerActionState {
    pub(crate) fn new(account_scope: String) -> Self {
        Self {
            account_scope,
            session_generation: 0,
            pending_feedback: HashSet::new(),
        }
    }

    pub(crate) fn set_account_scope(&mut self, account_scope: String) -> bool {
        if self.account_scope == account_scope {
            return false;
        }
        self.account_scope = account_scope;
        self.session_generation = self.session_generation.wrapping_add(1);
        self.pending_feedback.clear();
        true
    }

    pub(crate) fn begin_feedback(
        &mut self,
        key: DeezerFeedbackKey,
    ) -> Option<DeezerFeedbackTicket> {
        if !self.pending_feedback.insert(key.clone()) {
            return None;
        }
        Some(DeezerFeedbackTicket {
            key,
            account_scope: self.account_scope.clone(),
            session_generation: self.session_generation,
        })
    }

    pub(crate) fn finish_feedback(&mut self, ticket: &DeezerFeedbackTicket) -> bool {
        if ticket.session_generation != self.session_generation
            || ticket.account_scope != self.account_scope
        {
            return false;
        }
        self.pending_feedback.remove(&ticket.key)
    }

    #[cfg(test)]
    pub(crate) fn feedback_pending(&self, key: &DeezerFeedbackKey) -> bool {
        self.pending_feedback.contains(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key() -> DeezerFeedbackKey {
        DeezerFeedbackKey {
            kind: DeezerFeedbackKind::Song,
            id: "42".into(),
        }
    }

    #[test]
    fn duplicate_feedback_is_suppressed_until_completion() {
        let key = key();
        let mut state = DeezerActionState::new("account-a".into());
        let ticket = state.begin_feedback(key.clone()).unwrap();
        assert!(state.begin_feedback(key.clone()).is_none());
        assert!(state.feedback_pending(&key));
        assert!(state.finish_feedback(&ticket));
        assert!(!state.feedback_pending(&key));
        assert!(state.begin_feedback(key).is_some());
    }

    #[test]
    fn changing_account_scope_clears_pending_feedback_and_allows_the_same_key() {
        let key = key();
        let mut state = DeezerActionState::new("account-a".into());
        let old_ticket = state.begin_feedback(key.clone()).unwrap();

        assert!(state.set_account_scope("account-b".into()));
        assert!(!state.feedback_pending(&key));
        let new_ticket = state.begin_feedback(key.clone()).unwrap();
        assert!(!state.finish_feedback(&old_ticket));
        assert!(state.feedback_pending(&key));
        assert!(state.finish_feedback(&new_ticket));
    }

    #[test]
    fn returning_to_an_account_does_not_accept_a_stale_session_ticket() {
        let key = key();
        let mut state = DeezerActionState::new("account-a".into());
        let old_ticket = state.begin_feedback(key.clone()).unwrap();
        state.set_account_scope("account-b".into());
        state.set_account_scope("account-a".into());
        let new_ticket = state.begin_feedback(key.clone()).unwrap();

        assert!(!state.finish_feedback(&old_ticket));
        assert!(state.feedback_pending(&key));
        assert!(state.finish_feedback(&new_ticket));
    }
}

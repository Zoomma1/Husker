//! Options bollard pour le streaming live des logs d'un container (HUSKER-24).

use bollard::query_parameters::{LogsOptions, LogsOptionsBuilder};

/// Lignes d'historique envoyées à la connexion avant de basculer en suivi live.
/// Fixe pour le MVP — pas de scope pour une configuration par client (hors scope du ticket).
pub const DEFAULT_TAIL_LINES: u32 = 100;

/// `tail` lignes d'historique puis `follow`, stdout+stderr confondus.
pub fn tail_then_follow_options(tail: u32) -> LogsOptions {
    LogsOptionsBuilder::new()
        .follow(true)
        .stdout(true)
        .stderr(true)
        .tail(&tail.to_string())
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tail_then_follow_options_sets_follow_both_streams_and_tail() {
        let opts = tail_then_follow_options(100);
        assert!(opts.follow);
        assert!(opts.stdout);
        assert!(opts.stderr);
        assert_eq!(opts.tail, "100");
    }

    #[test]
    fn tail_then_follow_options_formats_tail_as_plain_integer() {
        let opts = tail_then_follow_options(42);
        assert_eq!(opts.tail, "42");
    }
}

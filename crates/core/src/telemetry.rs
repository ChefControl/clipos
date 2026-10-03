use tracing::Subscriber;
use tracing_subscriber::{EnvFilter, fmt, fmt::MakeWriter, prelude::*};

/// Log output format. JSON in Azure (App Service ships stdout to Log Analytics),
/// human-readable locally.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    #[default]
    Pretty,
    Json,
}

impl std::str::FromStr for LogFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "pretty" => Ok(Self::Pretty),
            "json" => Ok(Self::Json),
            other => Err(format!(
                "unknown log format {other:?} (expected pretty or json)"
            )),
        }
    }
}

/// Installs the global tracing subscriber. Filter comes from `RUST_LOG`, defaulting to
/// `info` for everything and `debug` for clipos crates.
pub fn init(format: LogFormat) {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(DEFAULT_FILTER));
    subscriber(format, filter, std::io::stdout).init();
}

const DEFAULT_FILTER: &str = "info,clipos_core=debug,clipos_api=debug,clipos_worker=debug";

/// The subscriber `init` installs, writing to `writer`.
fn subscriber<W>(format: LogFormat, filter: EnvFilter, writer: W) -> impl Subscriber + Send + Sync
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let layer = match format {
        LogFormat::Pretty => fmt::layer().with_writer(writer).boxed(),
        LogFormat::Json => fmt::layer()
            .json()
            .flatten_event(true)
            .with_current_span(false)
            .with_writer(writer)
            .boxed(),
    };
    tracing_subscriber::registry().with(filter).with(layer)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    /// Collects what the subscriber writes.
    #[derive(Clone, Default)]
    struct Captured(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for Captured {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl Captured {
        fn lines(&self) -> Vec<String> {
            String::from_utf8(self.0.lock().unwrap().clone())
                .unwrap()
                .lines()
                .map(str::to_owned)
                .collect()
        }
    }

    /// Logs a few events through `init`'s subscriber with the default filter.
    fn log(format: LogFormat) -> Vec<String> {
        let out = Captured::default();
        let writer = out.clone();
        let subscriber = subscriber(format, EnvFilter::new(DEFAULT_FILTER), move || {
            writer.clone()
        });
        tracing::subscriber::with_default(subscriber, || {
            let span = tracing::info_span!("job", id = 7);
            let _entered = span.enter();
            tracing::info!(target: "clipos_worker", count = 3, "removed leftover temp dirs");
            tracing::debug!(target: "clipos_api::live", "kept: our crates log at debug");
            tracing::debug!(target: "sqlx::query", "dropped: others only from info");
        });
        out.lines()
    }

    #[test]
    fn json_logs_are_one_flat_object_per_event() {
        let lines = log(LogFormat::Json);
        assert_eq!(lines.len(), 2, "{lines:?}");

        let event: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(event["level"], "INFO");
        assert_eq!(event["target"], "clipos_worker");
        // Fields sit next to the message rather than under `fields`, and the current span
        // is left out (Log Analytics gets one flat row per event).
        assert_eq!(event["message"], "removed leftover temp dirs");
        assert_eq!(event["count"], 3);
        assert!(event.get("fields").is_none(), "{event}");
        assert!(event.get("span").is_none(), "{event}");
        assert!(event["timestamp"].is_string());

        let event: serde_json::Value = serde_json::from_str(&lines[1]).unwrap();
        assert_eq!(event["level"], "DEBUG");
        assert_eq!(event["target"], "clipos_api::live");
    }

    #[test]
    fn pretty_logs_are_text() {
        let lines = log(LogFormat::Pretty);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines[0].contains("removed leftover temp dirs"),
            "{}",
            lines[0]
        );
        assert!(lines[0].contains("count"), "{}", lines[0]);
        assert!(
            lines[1].contains("kept: our crates log at debug"),
            "{}",
            lines[1]
        );
        assert!(serde_json::from_str::<serde_json::Value>(&lines[0]).is_err());
    }

    #[test]
    fn log_format_parses() {
        assert_eq!("JSON".parse(), Ok(LogFormat::Json));
        assert_eq!("pretty".parse(), Ok(LogFormat::Pretty));
        assert_eq!(
            "xml".parse::<LogFormat>(),
            Err("unknown log format \"xml\" (expected pretty or json)".into())
        );
        assert_eq!(LogFormat::default(), LogFormat::Pretty);
    }
}

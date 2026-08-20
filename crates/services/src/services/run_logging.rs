use tracing::Span;
use uuid::Uuid;

/// Creates the span shared by all lifecycle logs for one execution process.
pub fn execution_run_span(run_id: Uuid) -> Span {
    tracing::info_span!("execution_run", run_id = %run_id)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing::{Subscriber, field::Visit, span::Attributes};
    use tracing_subscriber::{Layer, layer::Context, prelude::*};
    use uuid::Uuid;

    use super::execution_run_span;

    #[derive(Default)]
    struct FieldVisitor {
        run_id: Option<String>,
    }

    impl Visit for FieldVisitor {
        fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
            if field.name() == "run_id" {
                self.run_id = Some(format!("{value:?}").trim_matches('"').to_string());
            }
        }
    }

    struct RunIdLayer(Arc<Mutex<Vec<String>>>);

    impl<S: Subscriber> Layer<S> for RunIdLayer {
        fn on_new_span(&self, attrs: &Attributes<'_>, _: &tracing::span::Id, _: Context<'_, S>) {
            let mut visitor = FieldVisitor::default();
            attrs.record(&mut visitor);
            if let Some(run_id) = visitor.run_id {
                self.0.lock().unwrap().push(run_id);
            }
        }
    }

    #[test]
    fn execution_run_span_records_the_execution_process_id_as_run_id() {
        let expected_run_id = Uuid::new_v4();
        let captured_run_ids = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(RunIdLayer(captured_run_ids.clone()));

        tracing::subscriber::with_default(subscriber, || {
            let _span = execution_run_span(expected_run_id);
        });

        assert_eq!(
            captured_run_ids.lock().unwrap().as_slice(),
            &[expected_run_id.to_string()]
        );
    }
}

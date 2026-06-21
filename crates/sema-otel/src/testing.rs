//! Deterministic in-process span capture for tests (behind the `testing` feature).
//!
//! Installs an in-memory exporter as the global provider and enables the facade, so a
//! downstream crate can run real Sema/LLM code and assert on the emitted spans as
//! plain JSON — no network, no collector, no OTel types in the test.

use opentelemetry::global;
use opentelemetry_sdk::metrics::{InMemoryMetricExporter, PeriodicReader, SdkMeterProvider};
use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider};

use crate::file_exporter::span_to_json;

/// A handle to in-memory captured spans + metrics. Created by [`install`].
pub struct SpanCapture {
    exporter: InMemorySpanExporter,
    provider: SdkTracerProvider,
    metric_exporter: InMemoryMetricExporter,
    meter_provider: SdkMeterProvider,
}

/// Install in-memory span + metric exporters as the global providers and enable the
/// facade (traces + metrics).
///
/// Call ONCE per test process (the global providers are process-global). Put the test
/// in its own integration-test file so it gets a fresh process.
pub fn install() -> SpanCapture {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    global::set_tracer_provider(provider.clone());

    let metric_exporter = InMemoryMetricExporter::default();
    let reader = PeriodicReader::builder(metric_exporter.clone()).build();
    let meter_provider = SdkMeterProvider::builder().with_reader(reader).build();
    global::set_meter_provider(meter_provider.clone());

    // Force the facade on (it would otherwise only enable via init_from_env).
    super::use_host_global();
    super::enable_metrics();
    SpanCapture {
        exporter,
        provider,
        metric_exporter,
        meter_provider,
    }
}

impl SpanCapture {
    /// All finished spans so far, serialized to the Sema JSONL schema (one object per
    /// span). Flushes first so freshly-ended spans are visible.
    pub fn spans_json(&self) -> Vec<serde_json::Value> {
        let _ = self.provider.force_flush();
        self.exporter
            .get_finished_spans()
            .unwrap_or_default()
            .iter()
            .map(span_to_json)
            .collect()
    }

    /// Find the first captured span with the given `name`.
    pub fn span_named(&self, name: &str) -> Option<serde_json::Value> {
        self.spans_json().into_iter().find(|s| s["name"] == name)
    }

    /// Names of all recorded metric instruments (flushes the meter provider first).
    pub fn metric_names(&self) -> Vec<String> {
        let _ = self.meter_provider.force_flush();
        let mut names = Vec::new();
        for rm in self
            .metric_exporter
            .get_finished_metrics()
            .unwrap_or_default()
        {
            for sm in rm.scope_metrics() {
                for m in sm.metrics() {
                    names.push(m.name().to_string());
                }
            }
        }
        names
    }
}

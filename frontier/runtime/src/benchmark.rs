//! Dependency-free JSON serialization for reproducible benchmark records.

use crate::FrameSummary;

/// Captures the conditions and available measurements for one benchmark run.
#[derive(Clone, Debug, PartialEq)]
pub struct BenchmarkReport {
    pub scene: String,
    pub seed: u64,
    pub route: String,
    pub output_width: u32,
    pub output_height: u32,
    pub adapter: String,
    pub vsync: bool,
    /// Number of initial frames excluded from the reported frame-time summary.
    pub warmup_frames: u64,
    pub frames: FrameSummary,
    /// Isolated GPU duration in milliseconds; `None` means the renderer did
    /// not provide GPU timestamps and is serialized as JSON `null`.
    pub isolated_gpu_ms: Option<f64>,
    /// Optional peak/resident memory counters in bytes, when available.
    pub used_memory_bytes: Option<u64>,
    pub committed_memory_bytes: Option<u64>,
}

impl BenchmarkReport {
    /// Serialize a stable, compact JSON object without pulling in a runtime
    /// dependency. Optional/unavailable measurements are explicit `null`s.
    pub fn to_json(&self) -> String {
        format!(
            concat!(
                "{{\"schema\":1,\"scene\":{},\"seed\":{},\"route\":{},",
                "\"output\":{{\"width\":{},\"height\":{}}},",
                "\"adapter\":{},\"vsync\":{},",
                "\"warmup\":{{\"policy\":\"discard_first_frames\",\"frames\":{}}},",
                "\"frames\":{{\"sample_count\":{},\"accepted_frames\":{},",
                "\"missed_budget_frames\":{},\"rejected_inputs\":{},",
                "\"p50_ms\":{},\"p95_ms\":{},\"p99_ms\":{}}},",
                "\"isolated_gpu_ms\":{},\"used_memory_bytes\":{},",
                "\"committed_memory_bytes\":{}}}"
            ),
            quote(&self.scene),
            self.seed,
            quote(&self.route),
            self.output_width,
            self.output_height,
            quote(&self.adapter),
            self.vsync,
            self.warmup_frames,
            self.frames.sample_count,
            self.frames.accepted_frames,
            self.frames.missed_budget_frames,
            self.frames.rejected_inputs,
            optional_float(self.frames.p50_ms),
            optional_float(self.frames.p95_ms),
            optional_float(self.frames.p99_ms),
            optional_float(self.isolated_gpu_ms),
            optional_u64(self.used_memory_bytes),
            optional_u64(self.committed_memory_bytes),
        )
    }
}

fn optional_float(value: Option<f64>) -> String {
    match value {
        Some(v) if v.is_finite() => v.to_string(),
        _ => "null".to_owned(),
    }
}

fn optional_u64(value: Option<u64>) -> String {
    value.map_or_else(|| "null".to_owned(), |v| v.to_string())
}

fn quote(value: &str) -> String {
    let mut out = String::with_capacity(value.len() + 2);
    out.push('"');
    for ch in value.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\u{08}' => out.push_str("\\b"),
            '\u{0c}' => out.push_str("\\f"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if ch <= '\u{1f}' => out.push_str(&format!("\\u{:04x}", ch as u32)),
            ch => out.push(ch),
        }
    }
    out.push('"');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> BenchmarkReport {
        BenchmarkReport {
            scene: "ridge \"north\"\nvalley".to_owned(),
            seed: 42,
            route: "fly\\turn".to_owned(),
            output_width: 1920,
            output_height: 1080,
            adapter: "GPU\tModel".to_owned(),
            vsync: false,
            warmup_frames: 60,
            frames: FrameSummary {
                sample_count: 3,
                accepted_frames: 4,
                missed_budget_frames: 1,
                rejected_inputs: 0,
                p50_ms: Some(10.0),
                p95_ms: Some(15.0),
                p99_ms: Some(15.0),
            },
            isolated_gpu_ms: None,
            used_memory_bytes: Some(1234),
            committed_memory_bytes: None,
        }
    }

    #[test]
    fn json_escapes_text_and_emits_unavailable_measurements_as_null() {
        let json = report().to_json();
        assert!(json.contains(r#""scene":"ridge \"north\"\nvalley""#));
        assert!(json.contains(r#""route":"fly\\turn""#));
        assert!(json.contains(r#""adapter":"GPU\tModel""#));
        assert!(json.contains(r#""isolated_gpu_ms":null"#));
        assert!(json.contains(r#""committed_memory_bytes":null"#));
        assert!(json.contains(r#""policy":"discard_first_frames","frames":60"#));
    }

    #[test]
    fn invalid_optional_float_is_serialized_as_null() {
        let mut r = report();
        r.isolated_gpu_ms = Some(f64::NAN);
        r.frames.p99_ms = Some(f64::INFINITY);
        let json = r.to_json();
        assert!(json.contains(r#""isolated_gpu_ms":null"#));
        assert!(json.contains(r#""p99_ms":null"#));
    }
}

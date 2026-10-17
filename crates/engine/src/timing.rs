//! Per-command timings for script runs and batches (#1833): how long each command took and,
//! when asked, how much of it went to kernel booleans and tessellation. The CLI's `run
//! --timings`, MCP `batch` and the app's `engine.batch` all report steps the same way:
//! `{"index", "command", "ms", "profile"?: {"boolean_ms", "tessellate_ms", "other_ms"}}`.

use serde_json::{Value, json};

use crate::kernel::profile::{self, KernelTimes};

/// One step's time.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Step {
    /// Wall time, ms.
    pub ms: f64,
    /// Kernel time within it, when profiled.
    pub kernel: Option<KernelTimes>,
}

/// A clock for a whole run (`total_ms`).
pub struct Clock(web_time::Instant);

impl Clock {
    pub fn start() -> Clock {
        Clock(web_time::Instant::now())
    }
    pub fn ms(&self) -> f64 {
        self.0.elapsed().as_secs_f64() * 1000.0
    }
}

/// Run `f` and time it; with `profile`, its kernel time too.
pub fn time<T>(profile: bool, f: impl FnOnce() -> T) -> (T, Step) {
    let clock = Clock::start();
    if profile {
        let (r, k) = profile::capture(f);
        (r, Step { ms: clock.ms(), kernel: Some(k) })
    } else {
        let r = f();
        (r, Step { ms: clock.ms(), kernel: None })
    }
}

/// Milliseconds to the microsecond, so the JSON stays readable.
pub fn round_ms(ms: f64) -> f64 {
    if ms.is_finite() { (ms * 1000.0).round() / 1000.0 } else { 0.0 }
}

impl Step {
    /// The step's report entry (`index` from 0, as a batch's `failed.index`).
    pub fn to_json(&self, index: usize, command: &str) -> Value {
        let mut v = json!({"index": index, "command": command, "ms": round_ms(self.ms)});
        if let (Some(k), Some(o)) = (self.kernel, v.as_object_mut()) {
            let other = (self.ms - k.boolean_ms - k.tessellate_ms).max(0.0);
            o.insert(
                "profile".into(),
                json!({"boolean_ms": round_ms(k.boolean_ms), "tessellate_ms": round_ms(k.tessellate_ms), "other_ms": round_ms(other)}),
            );
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_report_their_time_and_profile_only_when_asked() {
        let (r, step) = time(false, || 7);
        assert_eq!(r, 7);
        let v = step.to_json(2, "solid.box");
        assert_eq!((v["index"].as_u64(), v["command"].as_str()), (Some(2), Some("solid.box")));
        assert!(v["ms"].as_f64().is_some_and(|ms| ms >= 0.0));
        assert!(v.get("profile").is_none());
        let step = Step { ms: 10.0, kernel: Some(KernelTimes { boolean_ms: 6.0, tessellate_ms: 3.0 }) };
        assert_eq!(step.to_json(0, "x")["profile"], json!({"boolean_ms": 6.0, "tessellate_ms": 3.0, "other_ms": 1.0}));
        // Rounding can make the parts add up to a hair more than the whole: never a negative rest.
        let step = Step { ms: 1.0, kernel: Some(KernelTimes { boolean_ms: 1.0000004, tessellate_ms: 0.0 }) };
        assert_eq!(step.to_json(0, "x")["profile"]["other_ms"], json!(0.0));
        assert_eq!(round_ms(f64::NAN), 0.0);
    }

    #[test]
    fn a_timed_script_reports_every_step_and_the_kernel_share_of_a_join() {
        let script = json!({"commands": [
            {"command": "solid.box", "params": {"length": 10, "width": 10, "height": 10}},
            {"command": "solid.box", "params": {"length": 4, "width": 4, "height": 30, "operation": "join"}},
        ]});
        let mut s = crate::Session::default();
        let (results, t) = s.run_script_timed(&script, true).unwrap();
        assert_eq!(results.len(), 2);
        let rows = t["timings"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1]["command"], "solid.box");
        assert_eq!(rows[1]["index"], 1);
        let p = &rows[1]["profile"];
        assert!(p["boolean_ms"].as_f64().unwrap() > 0.0, "{t}");
        let sum: f64 = ["boolean_ms", "tessellate_ms", "other_ms"].iter().map(|k| p[k].as_f64().unwrap()).sum();
        assert!((sum - rows[1]["ms"].as_f64().unwrap()).abs() < 0.01, "{t}");
        let total = t["total_ms"].as_f64().unwrap();
        assert!(total + 0.01 >= rows.iter().map(|r| r["ms"].as_f64().unwrap()).sum::<f64>(), "{t}");
        // Without profile: the same rows, no kernel split; the plain run is unchanged.
        let (_, t) = crate::Session::default().run_script_timed(&script, false).unwrap();
        assert!(t["timings"][0].get("profile").is_none());
        assert_eq!(crate::Session::default().run_script(&script).unwrap().len(), 2);
        // A failing step still fails the run, with its step number.
        let bad = json!([{"command": "solid.box", "params": {"length": -1}}]);
        let e = crate::Session::default().run_script_timed(&bad, true).unwrap_err().to_string();
        assert!(e.contains("step 1"), "{e}");
    }
}

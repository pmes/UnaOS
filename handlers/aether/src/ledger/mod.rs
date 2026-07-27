use std::cell::RefCell;
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

thread_local! {
    /// The live ledger for this thread. The engine, JS bindings, and CSS
    /// cascade all run on one thread (same idiom as js::DOM_STATE), so a
    /// thread-local is the honest single collection point.
    static LEDGER: RefCell<ApiCoverageLedger> = RefCell::new(ApiCoverageLedger::new());
}

/// Records a missing DOM API on the thread's live ledger.
pub fn record_dom(api_name: &str) {
    LEDGER.with(|l| l.borrow_mut().record_dom(api_name));
}

/// Records a missing CSS feature on the thread's live ledger.
pub fn record_css(api_name: &str) {
    LEDGER.with(|l| l.borrow_mut().record_css(api_name));
}

/// Records a missing JS API on the thread's live ledger.
pub fn record_js(api_name: &str) {
    LEDGER.with(|l| l.borrow_mut().record_js(api_name));
}

/// Returns a snapshot of the thread's live ledger.
pub fn snapshot() -> ApiCoverageLedger {
    LEDGER.with(|l| l.borrow().clone())
}

/// Clears the thread's live ledger (test isolation, or per-page resets).
pub fn reset() {
    LEDGER.with(|l| *l.borrow_mut() = ApiCoverageLedger::new());
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ApiCategory {
    Dom,
    Css,
    Js,
}

impl std::fmt::Display for ApiCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiCategory::Dom => write!(f, "DOM"),
            ApiCategory::Css => write!(f, "CSS"),
            ApiCategory::Js => write!(f, "JS"),
        }
    }
}

/// A ledger to record unsupported/unimplemented API calls during
/// layout and JavaScript execution. This implements the "honest failure"
/// system to avoid silently ignoring missing functionality.
#[derive(Debug, Default, Clone)]
pub struct ApiCoverageLedger {
    /// Maps (Category, API Name) to the number of times it was called.
    /// Using BTreeMap so the dump is sorted automatically by category then name.
    missing_apis: BTreeMap<(ApiCategory, String), usize>,
}

impl ApiCoverageLedger {
    /// Creates a new, empty API coverage ledger.
    pub fn new() -> Self {
        Self::default()
    }

    /// Records an unimplemented API call.
    pub fn record(&mut self, category: ApiCategory, api_name: &str) {
        let key = (category, api_name.to_string());
        *self.missing_apis.entry(key).or_insert(0) += 1;
    }

    /// Convenience method for recording a missing DOM API.
    pub fn record_dom(&mut self, api_name: &str) {
        self.record(ApiCategory::Dom, api_name);
    }

    /// Convenience method for recording a missing CSS API.
    pub fn record_css(&mut self, api_name: &str) {
        self.record(ApiCategory::Css, api_name);
    }

    /// Convenience method for recording a missing JS API.
    pub fn record_js(&mut self, api_name: &str) {
        self.record(ApiCategory::Js, api_name);
    }

    /// Number of distinct missing APIs recorded.
    pub fn len(&self) -> usize {
        self.missing_apis.len()
    }

    /// True when nothing has been recorded.
    pub fn is_empty(&self) -> bool {
        self.missing_apis.is_empty()
    }

    /// True if the given API name was recorded under the given category.
    pub fn contains(&self, category: ApiCategory, api_name: &str) -> bool {
        self.missing_apis.contains_key(&(category, api_name.to_string()))
    }

    /// Dumps the current ledger state to a file in a human-readable format.
    pub fn dump_to_file<P: AsRef<Path>>(&self, path: P) -> io::Result<()> {
        let mut file = File::create(path)?;
        writeln!(file, "Aether M5 API Ledger: Honest Failures")?;
        writeln!(file, "=====================================")?;
        writeln!(file, "{:<8} | {:<40} | {}", "Category", "API Name", "Call Count")?;
        writeln!(file, "----------------------------------------------------------------------")?;
        
        for ((category, name), count) in &self.missing_apis {
            writeln!(file, "{:<8} | {:<40} | {}", category.to_string(), name, count)?;
        }
        
        Ok(())
    }
}

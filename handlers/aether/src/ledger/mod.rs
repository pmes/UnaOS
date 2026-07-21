use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Write};
use std::path::Path;

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

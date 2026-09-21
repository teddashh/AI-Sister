//! Local usage adapters.
//!
//! Production reads only an explicit configured directory of `*.jsonl` session
//! files. It never expands `$HOME`, never opens auth/credential files, and never
//! infers remaining quota from spent tokens. Tests use synthetic fixtures.

use crate::model::{LocalAdapterKind, LocalProductUsage, LocalUsage, LocalUsageReport, Measured};
#[cfg(test)]
use crate::model::{ProductId, UsageAmount};
use crate::sessions::{LocalReadRequest, read_sessions};
use std::path::{Path, PathBuf};

pub trait LocalUsageAdapter {
    fn read(&self) -> LocalUsage {
        self.report().primary()
    }

    fn report(&self) -> LocalUsageReport;
}

#[derive(Debug, Clone, Copy, Default)]
pub struct UnavailableLocalUsage;

impl LocalUsageAdapter for UnavailableLocalUsage {
    fn report(&self) -> LocalUsageReport {
        LocalUsageReport::disabled()
    }
}

/// Explicit opt-in directory. The path must already be absolute; this type does
/// not search the user home directory.
#[derive(Debug, Clone)]
pub struct ConfiguredSessionAdapter {
    pub enabled: bool,
    pub root: Option<PathBuf>,
}

impl ConfiguredSessionAdapter {
    pub fn from_config(enabled: bool, root: &str) -> Self {
        let trimmed = root.trim();
        Self {
            enabled,
            root: if trimmed.is_empty() {
                None
            } else {
                Some(Path::new(trimmed).to_path_buf())
            },
        }
    }
}

impl LocalUsageAdapter for ConfiguredSessionAdapter {
    fn report(&self) -> LocalUsageReport {
        read_sessions(LocalReadRequest {
            enabled: self.enabled,
            root: self.root.as_deref(),
        })
    }
}

/// Test fixture. Never constructed from real CLI files.
#[derive(Debug, Clone)]
pub struct SyntheticLocalUsage {
    pub usage: LocalUsage,
}

impl LocalUsageAdapter for SyntheticLocalUsage {
    fn report(&self) -> LocalUsageReport {
        let mut usage = self.usage.clone();
        usage.adapter = LocalAdapterKind::SyntheticFixture;
        let products = match usage.product {
            Some(product) => vec![LocalProductUsage {
                product,
                observed_tokens: usage.used,
                remaining_tokens: usage.remaining,
                quota: Measured::Unknown,
                observed_at_unix_ms: None,
                adapter: LocalAdapterKind::SyntheticFixture,
                provenance: "synthetic-fixture",
            }],
            None => Vec::new(),
        };
        LocalUsageReport {
            enabled: true,
            configured: true,
            products,
            skipped_auth_files: 0,
            files_found: 0,
            files_read: 0,
            files_skipped_large: 0,
            files_capped: 0,
            truncated_lines: 0,
            scan_complete: true,
            error: None,
            adapter: LocalAdapterKind::SyntheticFixture,
        }
    }
}

#[cfg(test)]
pub fn synthetic_observed_without_remaining(product: ProductId, used: u64) -> LocalUsage {
    LocalUsage {
        product: Some(product),
        used: Measured::Observed(UsageAmount::new(used)),
        remaining: Measured::Unknown,
        adapter: LocalAdapterKind::SyntheticFixture,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unavailable_adapter_is_unknown_not_zero() {
        let usage = UnavailableLocalUsage.read();
        assert_eq!(usage, LocalUsage::unavailable());
        assert!(matches!(usage.used, Measured::Unknown));
        assert!(matches!(usage.remaining, Measured::Unknown));
    }

    #[test]
    fn synthetic_fixture_keeps_observed_and_remaining_apart() {
        let usage = SyntheticLocalUsage {
            usage: synthetic_observed_without_remaining(ProductId::Codex, 12),
        }
        .read();
        assert_eq!(usage.used, Measured::Observed(UsageAmount::new(12)));
        assert!(usage.remaining.is_unknown());
        assert_eq!(usage.adapter, LocalAdapterKind::SyntheticFixture);
    }
}

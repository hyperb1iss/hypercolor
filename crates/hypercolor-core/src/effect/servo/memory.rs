use profile_traits::mem::{MemoryReportResult, ReportKind};
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ServoMemoryReportSnapshot {
    pub processes: Vec<ServoProcessMemoryReport>,
    pub totals: ServoMemoryReportTotals,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ServoProcessMemoryReport {
    pub pid: u32,
    pub is_main_process: bool,
    pub reports: Vec<ServoMemoryReport>,
    pub totals: ServoMemoryReportTotals,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ServoMemoryReport {
    pub path: String,
    pub path_components: Vec<String>,
    pub kind: ServoMemoryReportKind,
    pub bytes: u64,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ServoMemoryReportKind {
    ExplicitJemallocHeapSize,
    ExplicitSystemHeapSize,
    ExplicitNonHeapSize,
    ExplicitUnknownLocationSize,
    NonExplicitSize,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct ServoMemoryReportTotals {
    pub report_count: usize,
    pub explicit_bytes: u64,
    pub explicit_jemalloc_heap_bytes: u64,
    pub explicit_system_heap_bytes: u64,
    pub explicit_non_heap_bytes: u64,
    pub explicit_unknown_location_bytes: u64,
    pub non_explicit_bytes: u64,
}

impl ServoMemoryReportSnapshot {
    pub(super) fn from_servo_result(result: MemoryReportResult) -> Self {
        let mut snapshot = Self {
            processes: Vec::with_capacity(result.results.len()),
            totals: ServoMemoryReportTotals::default(),
        };

        for process in result.results {
            let mut process_report = ServoProcessMemoryReport {
                pid: process.pid,
                is_main_process: process.is_main_process,
                reports: Vec::with_capacity(process.reports.len()),
                totals: ServoMemoryReportTotals::default(),
            };

            for report in process.reports {
                let kind = ServoMemoryReportKind::from_report_kind(&report.kind);
                let bytes = u64::try_from(report.size).unwrap_or(u64::MAX);
                process_report.totals.add(kind, bytes);
                process_report.reports.push(ServoMemoryReport {
                    path: memory_report_path(&report.path),
                    path_components: report.path,
                    kind,
                    bytes,
                });
            }

            snapshot.totals.merge(&process_report.totals);
            snapshot.processes.push(process_report);
        }

        snapshot
    }
}

impl ServoMemoryReportKind {
    const fn from_report_kind(kind: &ReportKind) -> Self {
        match kind {
            ReportKind::ExplicitJemallocHeapSize => Self::ExplicitJemallocHeapSize,
            ReportKind::ExplicitSystemHeapSize => Self::ExplicitSystemHeapSize,
            ReportKind::ExplicitNonHeapSize => Self::ExplicitNonHeapSize,
            ReportKind::ExplicitUnknownLocationSize => Self::ExplicitUnknownLocationSize,
            ReportKind::NonExplicitSize => Self::NonExplicitSize,
        }
    }

    const fn is_explicit(self) -> bool {
        !matches!(self, Self::NonExplicitSize)
    }
}

impl ServoMemoryReportTotals {
    fn add(&mut self, kind: ServoMemoryReportKind, bytes: u64) {
        self.report_count = self.report_count.saturating_add(1);
        if kind.is_explicit() {
            self.explicit_bytes = self.explicit_bytes.saturating_add(bytes);
        }
        match kind {
            ServoMemoryReportKind::ExplicitJemallocHeapSize => {
                self.explicit_jemalloc_heap_bytes =
                    self.explicit_jemalloc_heap_bytes.saturating_add(bytes);
            }
            ServoMemoryReportKind::ExplicitSystemHeapSize => {
                self.explicit_system_heap_bytes =
                    self.explicit_system_heap_bytes.saturating_add(bytes);
            }
            ServoMemoryReportKind::ExplicitNonHeapSize => {
                self.explicit_non_heap_bytes = self.explicit_non_heap_bytes.saturating_add(bytes);
            }
            ServoMemoryReportKind::ExplicitUnknownLocationSize => {
                self.explicit_unknown_location_bytes =
                    self.explicit_unknown_location_bytes.saturating_add(bytes);
            }
            ServoMemoryReportKind::NonExplicitSize => {
                self.non_explicit_bytes = self.non_explicit_bytes.saturating_add(bytes);
            }
        }
    }

    fn merge(&mut self, other: &Self) {
        self.report_count = self.report_count.saturating_add(other.report_count);
        self.explicit_bytes = self.explicit_bytes.saturating_add(other.explicit_bytes);
        self.explicit_jemalloc_heap_bytes = self
            .explicit_jemalloc_heap_bytes
            .saturating_add(other.explicit_jemalloc_heap_bytes);
        self.explicit_system_heap_bytes = self
            .explicit_system_heap_bytes
            .saturating_add(other.explicit_system_heap_bytes);
        self.explicit_non_heap_bytes = self
            .explicit_non_heap_bytes
            .saturating_add(other.explicit_non_heap_bytes);
        self.explicit_unknown_location_bytes = self
            .explicit_unknown_location_bytes
            .saturating_add(other.explicit_unknown_location_bytes);
        self.non_explicit_bytes = self
            .non_explicit_bytes
            .saturating_add(other.non_explicit_bytes);
    }
}

fn memory_report_path(components: &[String]) -> String {
    components.join("/")
}

#[cfg(test)]
mod tests {
    use profile_traits::mem::{MemoryReport, Report};

    use super::*;

    #[test]
    fn converts_servo_memory_report_totals() {
        let snapshot = ServoMemoryReportSnapshot::from_servo_result(MemoryReportResult {
            results: vec![MemoryReport {
                pid: 42,
                is_main_process: true,
                reports: vec![
                    Report {
                        path: vec!["explicit".to_owned(), "layout".to_owned()],
                        kind: ReportKind::ExplicitJemallocHeapSize,
                        size: 64,
                    },
                    Report {
                        path: vec!["resident".to_owned()],
                        kind: ReportKind::NonExplicitSize,
                        size: 256,
                    },
                ],
            }],
        });

        assert_eq!(snapshot.totals.report_count, 2);
        assert_eq!(snapshot.totals.explicit_bytes, 64);
        assert_eq!(snapshot.totals.explicit_jemalloc_heap_bytes, 64);
        assert_eq!(snapshot.totals.non_explicit_bytes, 256);
        assert_eq!(snapshot.processes[0].reports[0].path, "explicit/layout");
    }
}

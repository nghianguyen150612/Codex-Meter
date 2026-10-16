//! Versioned benchmark identities and plan metadata.

use std::fmt;

use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use time::{format_description::well_known::Rfc3339, OffsetDateTime, UtcOffset};

use crate::telemetry::{
    ConfigurationIdentity, ConfigurationValue, MetricAvailability, MetricProvenance, QuotaMeterType,
};

/// Version of plan semantics, preflight policy, lifecycle, and evidence linkage.
pub const BENCHMARK_FRAMEWORK_VERSION: &str = "1.0.0";

/// Highest supported number of planned runs in one experiment.
pub const MAX_PLANNED_RUNS: u16 = 100;

const MAX_ID_LENGTH: usize = 64;
const MAX_CONFIGURATION_VALUE_LENGTH: usize = 128;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityValidationError {
    Empty,
    TooLong,
    InvalidCharacters,
}

impl fmt::Display for IdentityValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::Empty => "benchmark identity is empty",
            Self::TooLong => "benchmark identity exceeds the supported length",
            Self::InvalidCharacters => "benchmark identity has an invalid format",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for IdentityValidationError {}

macro_rules! opaque_identifier {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn parse(value: impl Into<String>) -> Result<Self, IdentityValidationError> {
                let value = value.into();
                validate_opaque_identifier(&value)?;
                Ok(Self(value))
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
            where
                D: Deserializer<'de>,
            {
                let value = String::deserialize(deserializer)?;
                Self::parse(value).map_err(D::Error::custom)
            }
        }
    };
}

opaque_identifier!(ExperimentId);
opaque_identifier!(RunId);
opaque_identifier!(WorkloadKey);
opaque_identifier!(AuthorizationId);

fn validate_opaque_identifier(value: &str) -> Result<(), IdentityValidationError> {
    if value.is_empty() {
        return Err(IdentityValidationError::Empty);
    }
    if value.len() > MAX_ID_LENGTH {
        return Err(IdentityValidationError::TooLong);
    }
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return Err(IdentityValidationError::Empty);
    };
    if !first.is_ascii_alphabetic()
        || !chars.all(|character| {
            character.is_ascii_alphanumeric() || character == '-' || character == '_'
        })
    {
        return Err(IdentityValidationError::InvalidCharacters);
    }
    Ok(())
}

/// Immutable-by-contract declaration for one repeated-workload experiment.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BenchmarkPlan {
    framework_version: String,
    experiment_id: ExperimentId,
    configuration: ConfigurationIdentity,
    workload_key: WorkloadKey,
    planned_run_count: u16,
    target_quota_meters: Vec<QuotaMeterType>,
    created_at: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BenchmarkPlanWire {
    framework_version: String,
    experiment_id: ExperimentId,
    configuration: ConfigurationIdentity,
    workload_key: WorkloadKey,
    planned_run_count: u16,
    target_quota_meters: Vec<QuotaMeterType>,
    created_at: String,
}

impl<'de> Deserialize<'de> for BenchmarkPlan {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = BenchmarkPlanWire::deserialize(deserializer)?;
        Self::new(
            wire.framework_version,
            wire.experiment_id,
            wire.configuration,
            wire.workload_key,
            wire.planned_run_count,
            wire.target_quota_meters,
            wire.created_at,
        )
        .map_err(D::Error::custom)
    }
}

impl BenchmarkPlan {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        framework_version: impl Into<String>,
        experiment_id: ExperimentId,
        configuration: ConfigurationIdentity,
        workload_key: WorkloadKey,
        planned_run_count: u16,
        target_quota_meters: Vec<QuotaMeterType>,
        created_at: impl Into<String>,
    ) -> Result<Self, PlanValidationError> {
        let plan = Self {
            framework_version: framework_version.into(),
            experiment_id,
            configuration,
            workload_key,
            planned_run_count,
            target_quota_meters,
            created_at: created_at.into(),
        };
        plan.validate()?;
        Ok(plan)
    }

    pub fn validate(&self) -> Result<(), PlanValidationError> {
        if self.framework_version != BENCHMARK_FRAMEWORK_VERSION {
            return Err(PlanValidationError::UnsupportedFrameworkVersion);
        }
        validate_configuration(&self.configuration)
            .map_err(|_| PlanValidationError::InvalidConfiguration)?;
        if self.planned_run_count == 0 || self.planned_run_count > MAX_PLANNED_RUNS {
            return Err(PlanValidationError::PlannedRunCountOutOfRange);
        }
        if self.target_quota_meters.is_empty() {
            return Err(PlanValidationError::NoTargetQuotaMeters);
        }
        for (index, meter) in self.target_quota_meters.iter().enumerate() {
            if self.target_quota_meters[index + 1..].contains(meter) {
                return Err(PlanValidationError::DuplicateQuotaMeter);
            }
        }
        parse_utc_timestamp(&self.created_at).map_err(|_| PlanValidationError::InvalidTimestamp)?;
        Ok(())
    }

    pub fn framework_version(&self) -> &str {
        &self.framework_version
    }

    pub fn experiment_id(&self) -> &ExperimentId {
        &self.experiment_id
    }

    pub fn configuration(&self) -> &ConfigurationIdentity {
        &self.configuration
    }

    pub fn workload_key(&self) -> &WorkloadKey {
        &self.workload_key
    }

    pub fn planned_run_count(&self) -> u16 {
        self.planned_run_count
    }

    pub fn target_quota_meters(&self) -> &[QuotaMeterType] {
        &self.target_quota_meters
    }

    pub fn created_at(&self) -> &str {
        &self.created_at
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlanValidationError {
    UnsupportedFrameworkVersion,
    InvalidConfiguration,
    PlannedRunCountOutOfRange,
    NoTargetQuotaMeters,
    DuplicateQuotaMeter,
    InvalidTimestamp,
}

impl fmt::Display for PlanValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let message = match self {
            Self::UnsupportedFrameworkVersion => "unsupported benchmark framework version",
            Self::InvalidConfiguration => "benchmark configuration identity is invalid",
            Self::PlannedRunCountOutOfRange => "planned run count is outside the supported range",
            Self::NoTargetQuotaMeters => "benchmark plan has no target quota meters",
            Self::DuplicateQuotaMeter => "benchmark plan has duplicate quota meters",
            Self::InvalidTimestamp => "benchmark plan timestamp is invalid",
        };
        formatter.write_str(message)
    }
}

impl std::error::Error for PlanValidationError {}

pub(crate) fn validate_configuration(configuration: &ConfigurationIdentity) -> Result<(), ()> {
    for value in [
        &configuration.plan,
        &configuration.model,
        &configuration.reasoning_level,
        &configuration.speed_mode,
        &configuration.codex_version,
    ] {
        validate_configuration_value(value)?;
    }
    Ok(())
}

fn validate_configuration_value(value: &ConfigurationValue) -> Result<(), ()> {
    match (value.availability, value.value.as_deref(), value.provenance) {
        (
            MetricAvailability::Available,
            Some(value),
            MetricProvenance::Observed | MetricProvenance::Derived,
        ) if is_configuration_token(value) => Ok(()),
        (MetricAvailability::Unavailable, None, MetricProvenance::Unavailable) => Ok(()),
        _ => Err(()),
    }
}

fn is_configuration_token(value: &str) -> bool {
    if value.is_empty() || value.len() > MAX_CONFIGURATION_VALUE_LENGTH {
        return false;
    }
    let mut bytes = value.bytes();
    let Some(first) = bytes.next() else {
        return false;
    };
    first.is_ascii_alphanumeric()
        && bytes
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-' | b'+'))
}

pub(crate) fn parse_utc_timestamp(value: &str) -> Result<OffsetDateTime, ()> {
    if !value.ends_with('Z') {
        return Err(());
    }
    let timestamp = OffsetDateTime::parse(value, &Rfc3339).map_err(|_| ())?;
    if timestamp.offset() != UtcOffset::UTC || !(0..=9999).contains(&timestamp.year()) {
        return Err(());
    }
    Ok(timestamp)
}

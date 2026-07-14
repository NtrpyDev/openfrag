pub const OFR_V1_FORMULA: &str = "ofr-1.0.0";

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CalculationIdentity {
    demo_hash: String,
    parser_build: String,
    generated_proto_build: String,
    metric_definition_version: String,
}

impl CalculationIdentity {
    pub fn ofr_v1(
        demo_hash: impl Into<String>,
        parser_build: impl Into<String>,
        generated_proto_build: impl Into<String>,
        metric_definition_version: impl Into<String>,
    ) -> Self {
        Self {
            demo_hash: demo_hash.into(),
            parser_build: parser_build.into(),
            generated_proto_build: generated_proto_build.into(),
            metric_definition_version: metric_definition_version.into(),
        }
    }

    pub fn demo_hash(&self) -> &str {
        &self.demo_hash
    }

    pub fn parser_build(&self) -> &str {
        &self.parser_build
    }

    pub fn generated_proto_build(&self) -> &str {
        &self.generated_proto_build
    }

    pub fn metric_definition_version(&self) -> &str {
        &self.metric_definition_version
    }

    pub const fn formula_version(&self) -> &'static str {
        OFR_V1_FORMULA
    }
}

#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct CalculationSeriesIdentity {
    local_player_id: String,
    parser_build: String,
    generated_proto_build: String,
    metric_definition_version: String,
    evidence_semantics_epoch: String,
}

impl CalculationSeriesIdentity {
    pub fn ofr_v1(
        local_player_id: impl Into<String>,
        parser_build: impl Into<String>,
        generated_proto_build: impl Into<String>,
        metric_definition_version: impl Into<String>,
        evidence_semantics_epoch: impl Into<String>,
    ) -> Self {
        Self {
            local_player_id: local_player_id.into(),
            parser_build: parser_build.into(),
            generated_proto_build: generated_proto_build.into(),
            metric_definition_version: metric_definition_version.into(),
            evidence_semantics_epoch: evidence_semantics_epoch.into(),
        }
    }

    pub fn local_player_id(&self) -> &str {
        &self.local_player_id
    }

    pub fn parser_build(&self) -> &str {
        &self.parser_build
    }

    pub fn generated_proto_build(&self) -> &str {
        &self.generated_proto_build
    }

    pub fn metric_definition_version(&self) -> &str {
        &self.metric_definition_version
    }

    pub fn evidence_semantics_epoch(&self) -> &str {
        &self.evidence_semantics_epoch
    }

    pub const fn formula_version(&self) -> &'static str {
        OFR_V1_FORMULA
    }

    pub fn differs_only_by_parser_or_proto(&self, other: &Self) -> bool {
        self.local_player_id == other.local_player_id
            && self.metric_definition_version == other.metric_definition_version
            && self.evidence_semantics_epoch == other.evidence_semantics_epoch
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_identities_cannot_select_an_alternate_formula() {
        let identity = CalculationIdentity::ofr_v1("demo", "parser", "proto", "metrics");
        assert_eq!(identity.formula_version(), OFR_V1_FORMULA);
    }
}

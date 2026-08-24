#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CanonicalMemoryKind {
    Wiki,
    Lens,
    Evidence,
    Decision,
}

impl CanonicalMemoryKind {
    pub const ALL: [Self; 4] = [Self::Wiki, Self::Lens, Self::Evidence, Self::Decision];

    pub const fn slug(self) -> &'static str {
        match self {
            Self::Wiki => "wiki",
            Self::Lens => "lens",
            Self::Evidence => "evidence",
            Self::Decision => "decision",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Wiki => "Wiki",
            Self::Lens => "Lens",
            Self::Evidence => "Evidence",
            Self::Decision => "Decision",
        }
    }

    pub fn from_slug(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.slug() == value)
    }

    pub fn from_label(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.label() == value)
    }

    pub fn parse_record_id(value: &str) -> Option<(Self, &str)> {
        if value.is_empty() || value.len() > 512 || value.trim() != value {
            return None;
        }
        let (kind, suffix) = value.split_once(':')?;
        let kind = Self::from_slug(kind)?;
        if suffix.split('/').any(|segment| {
            segment.is_empty()
                || segment.len() > 128
                || segment.trim() != segment
                || matches!(segment, "." | "..")
                || segment.contains(['\\', ':'])
                || segment.chars().any(char::is_control)
        }) {
            return None;
        }
        Some((kind, suffix))
    }
}

#[cfg(test)]
mod tests {
    use super::CanonicalMemoryKind;

    #[test]
    fn exact_kind_projections_round_trip() {
        for kind in CanonicalMemoryKind::ALL {
            assert_eq!(CanonicalMemoryKind::from_slug(kind.slug()), Some(kind));
            assert_eq!(CanonicalMemoryKind::from_label(kind.label()), Some(kind));
            assert_eq!(
                CanonicalMemoryKind::parse_record_id(&format!("{}:record", kind.slug())),
                Some((kind, "record"))
            );
        }
        assert_eq!(CanonicalMemoryKind::from_slug("Decision"), None);
        assert_eq!(CanonicalMemoryKind::from_label("decision"), None);
        assert_eq!(CanonicalMemoryKind::parse_record_id("skill:record"), None);
        assert_eq!(CanonicalMemoryKind::parse_record_id("method:record"), None);
        assert_eq!(CanonicalMemoryKind::parse_record_id("wiki:../record"), None);
    }
}

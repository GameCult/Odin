use anyhow::Result;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VerseObservation {
    pub verse_id: String,
    pub name: String,
    pub role: String,
    pub status: String,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceObservation {
    pub service_id: String,
    pub verse_id: String,
    pub name: String,
    pub state: String,
    pub detail: String,
    pub authority: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InterfaceObservation {
    pub provider_id: String,
    pub title: String,
    pub state: String,
    pub source: String,
    pub version: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ObservationStreamObservation {
    pub device_id: String,
    pub stream_id: String,
    pub kind: String,
    pub state: String,
    pub detail: String,
    pub owner: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TranslationRouteObservation {
    pub source_schema: String,
    pub target_schema: String,
    pub translation_kind: String,
    pub owner: String,
    pub version: String,
    pub notes: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GjallarAffordanceObservation {
    pub source_record: String,
    pub verse_id: Option<String>,
    pub surface_kind: String,
    pub action: String,
    pub authority: String,
    pub status: String,
    pub provenance: String,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OdinInputBatch {
    pub observed_at: String,
    pub source: String,
    pub verses: Vec<VerseObservation>,
    pub services: Vec<ServiceObservation>,
    pub interfaces: Vec<InterfaceObservation>,
    pub observation_streams: Vec<ObservationStreamObservation>,
    pub translation_routes: Vec<TranslationRouteObservation>,
    pub gjallar_affordances: Vec<GjallarAffordanceObservation>,
}

pub trait VerseIngest {
    fn observe_verses(&self) -> Result<Vec<VerseObservation>>;
}

pub trait ServiceIngest {
    fn observe_services(&self) -> Result<Vec<ServiceObservation>>;
}

pub trait InterfaceIngest {
    fn observe_interfaces(&self) -> Result<Vec<InterfaceObservation>>;
}

pub trait ObservationStreamIngest {
    fn observe_streams(&self) -> Result<Vec<ObservationStreamObservation>>;
}

pub trait TranslationRouteIngest {
    fn observe_translation_routes(&self) -> Result<Vec<TranslationRouteObservation>>;
}

pub trait GjallarAffordanceIngest {
    fn observe_gjallar_affordances(&self) -> Result<Vec<GjallarAffordanceObservation>>;
}

pub trait Clock {
    fn now(&self) -> String;
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FixedClock {
    now: String,
}

impl FixedClock {
    pub fn new(now: impl Into<String>) -> Self {
        Self { now: now.into() }
    }
}

impl Clock for FixedClock {
    fn now(&self) -> String {
        self.now.clone()
    }
}

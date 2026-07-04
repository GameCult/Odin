use crate::EveProviderAdvertisementRecord;
use cultmesh_rs::CultMeshNode;
use serde_json::Value;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OdinEndpointQuery<'a> {
    pub schema: Option<&'a str>,
    pub transport_contains: Option<&'a str>,
    pub host_hint: Option<&'a str>,
    pub device_filter: Option<&'a str>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OdinEndpointMatch {
    pub address: String,
    pub schema: Option<String>,
    pub transport: Option<String>,
    pub stream_id: Option<String>,
}

pub fn discover_provider_endpoints(
    node: &CultMeshNode,
    query: OdinEndpointQuery<'_>,
) -> Vec<OdinEndpointMatch> {
    let mut matches = Vec::new();
    for provider in node
        .cache()
        .get_all::<EveProviderAdvertisementRecord>()
        .ok()
        .into_iter()
        .flatten()
    {
        collect_provider_endpoint_matches(&provider.value, &query, &mut matches);
    }
    dedupe_endpoint_matches(matches)
}

fn collect_provider_endpoint_matches(
    value: &Value,
    query: &OdinEndpointQuery<'_>,
    matches: &mut Vec<OdinEndpointMatch>,
) {
    if query
        .host_hint
        .is_some_and(|hint| !provider_matches_host_hint(value, hint))
    {
        return;
    }
    collect_endpoint_matches_from_provider_value(value, query, matches);
}

fn collect_endpoint_matches_from_provider_value(
    value: &Value,
    query: &OdinEndpointQuery<'_>,
    matches: &mut Vec<OdinEndpointMatch>,
) {
    if let Some(streams) = value
        .get("inputStreams")
        .or_else(|| value.get("input_streams"))
        .and_then(|streams| streams.as_array())
    {
        for stream in streams {
            if let Some(endpoint_match) = endpoint_match_from_entry(stream, query, true) {
                matches.push(endpoint_match);
            }
        }
    }
    for collection_name in ["endpoints", "routes"] {
        if let Some(entries) = value
            .get(collection_name)
            .and_then(|entries| entries.as_array())
        {
            for entry in entries {
                if let Some(endpoint_match) = endpoint_match_from_entry(entry, query, false) {
                    matches.push(endpoint_match);
                }
            }
        }
    }
}

fn endpoint_match_from_entry(
    entry: &Value,
    query: &OdinEndpointQuery<'_>,
    allow_device_filter: bool,
) -> Option<OdinEndpointMatch> {
    let schema = entry
        .get("schema")
        .or_else(|| entry.get("schemaId"))
        .or_else(|| entry.get("schema_id"))
        .and_then(|value| value.as_str());
    if let Some(wanted_schema) = query.schema
        && schema != Some(wanted_schema)
    {
        return None;
    }
    let transport = entry.get("transport").and_then(|value| value.as_str());
    if let Some(wanted_transport) = query.transport_contains
        && !transport
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(&wanted_transport.to_ascii_lowercase())
    {
        return None;
    }
    if let Some(filter) = query.device_filter
        && allow_device_filter
        && !input_stream_matches_filter(entry, filter)
    {
        return None;
    }
    let address = entry.get("address").and_then(|value| value.as_str())?;
    if !endpoint_looks_like_socket(address) {
        return None;
    }
    Some(OdinEndpointMatch {
        address: address.to_string(),
        schema: schema.map(ToString::to_string),
        transport: transport.map(ToString::to_string),
        stream_id: entry
            .get("streamId")
            .or_else(|| entry.get("stream_id"))
            .and_then(|value| value.as_str())
            .map(ToString::to_string),
    })
}

fn input_stream_matches_filter(stream: &Value, filter: &str) -> bool {
    stream
        .get("streamId")
        .or_else(|| stream.get("stream_id"))
        .and_then(|value| value.as_str())
        .is_some_and(|stream_id| stream_id.contains(filter))
        || stream
            .get("devices")
            .and_then(|value| value.as_array())
            .is_some_and(|devices| {
                devices.iter().any(|device| {
                    device
                        .get("deviceId")
                        .or_else(|| device.get("device_id"))
                        .and_then(|value| value.as_str())
                        == Some(filter)
                        || device
                            .get("deviceKind")
                            .or_else(|| device.get("device_kind"))
                            .and_then(|value| value.as_str())
                            == Some(filter)
                })
            })
}

fn provider_matches_host_hint(value: &Value, host_hint: &str) -> bool {
    let normalized = host_hint
        .trim()
        .trim_end_matches(".local")
        .to_ascii_lowercase();
    if normalized.is_empty() {
        return true;
    }
    let explicit_identity = [
        "providerId",
        "provider_id",
        "id",
        "verseId",
        "verse_id",
        "hostId",
        "host_id",
        "runtimeId",
        "runtime_id",
        "locatedService",
        "located_service",
        "cultMeshAddress",
        "cult_mesh_address",
    ]
    .into_iter()
    .filter_map(|field| value.get(field).and_then(|value| value.as_str()));
    explicit_identity
        .map(normalized_host_fragment)
        .any(|identity| identity == normalized || identity.contains(&normalized))
}

fn normalized_host_fragment(value: &str) -> String {
    value.trim().trim_end_matches(".local").to_ascii_lowercase()
}

fn endpoint_looks_like_socket(endpoint: &str) -> bool {
    let Some((host, port)) = endpoint.rsplit_once(':') else {
        return false;
    };
    !host.trim().is_empty()
        && port
            .parse::<u16>()
            .is_ok_and(|parsed_port| parsed_port != 0)
}

fn dedupe_endpoint_matches(mut matches: Vec<OdinEndpointMatch>) -> Vec<OdinEndpointMatch> {
    matches.sort_by(|left, right| {
        (
            &left.address,
            &left.schema,
            &left.transport,
            &left.stream_id,
        )
            .cmp(&(
                &right.address,
                &right.schema,
                &right.transport,
                &right.stream_id,
            ))
    });
    matches.dedup();
    matches
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn host_hint_matches_explicit_provider_identity_not_nested_mentions() {
        let starfire_provider = json!({
            "providerId": "muninn.telemetry.starfire",
            "verseId": "starfire.local",
            "inputStreams": [{
                "address": "198.51.100.66:17888",
                "schema": "muninn.hid_controller_state.v1",
                "transport": "cultnet.transport.rudp.v0",
                "streamId": "raven:xbox-raven:hid-controller-state",
                "devices": [{
                    "deviceId": "xbox-raven",
                    "deviceKind": "xinput-controller"
                }]
            }]
        });

        let mut matches = Vec::new();
        collect_provider_endpoint_matches(
            &starfire_provider,
            &OdinEndpointQuery {
                schema: Some("muninn.hid_controller_state.v1"),
                transport_contains: Some("rudp"),
                host_hint: Some("raven"),
                device_filter: Some("xbox-raven"),
            },
            &mut matches,
        );

        assert!(matches.is_empty());
    }

    #[test]
    fn host_hint_matches_normalized_provider_catalog_identity() {
        let starfire_provider = json!({
            "id": "muninn.telemetry.starfire",
            "locatedService": "asgard.starfire.muninn",
            "routes": [{
                "address": "198.51.100.66:17888",
                "schema": "muninn.hid_controller_state.v1",
                "transport": "cultnet.transport.rudp.v0"
            }]
        });

        let mut matches = Vec::new();
        collect_provider_endpoint_matches(
            &starfire_provider,
            &OdinEndpointQuery {
                schema: Some("muninn.hid_controller_state.v1"),
                transport_contains: Some("rudp"),
                host_hint: Some("starfire"),
                device_filter: Some("nav-windows-psnav-0"),
            },
            &mut matches,
        );

        assert_eq!(matches.len(), 1);
        assert_eq!(matches[0].address, "198.51.100.66:17888");
    }
}

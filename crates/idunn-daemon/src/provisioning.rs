use anyhow::{Context, Result, anyhow, bail};
use cultcache_rs::{
    CultCacheEnvelope, CultCacheExpectedEnvelope, DatabaseEntry, SingleFileMessagePackBackingStore,
};
use cultnet_rs::{
    GameCultProviderHealthIdentity, IdunnServiceIdentity, ServiceIdentityProfile,
    ServiceIdentityTrustAnchor, derive_service_identity_id, enroll_service_identity_at,
    export_service_identity_trust_anchor, open_service_identity_at,
};
use odin_core::{
    GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA, GameCultServiceTrustAnchorRecord,
    IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SCHEMA,
    IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SIGNING_PURPOSE,
    IDUNN_DAEMON_HEALTH_TRUST_BINDING_SCHEMA, IdunnDaemonHealthTrustBindingRecord,
};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

trait RootDistributionDocument: DatabaseEntry {
    fn root_key(&self) -> &str;
}

impl RootDistributionDocument for IdunnDaemonHealthTrustBindingRecord {
    fn root_key(&self) -> &str {
        &self.binding_id
    }
}

impl RootDistributionDocument for GameCultServiceTrustAnchorRecord {
    fn root_key(&self) -> &str {
        &self.trust_anchor_id
    }
}

pub fn run(args: impl IntoIterator<Item = String>) -> Result<()> {
    let mut args = args.into_iter();
    let command = args.next().ok_or_else(|| anyhow!(usage()))?;
    let options = parse_options(args)?;
    match command.as_str() {
        "enroll-idunn-identity" => {
            require_only(&options, &["private-store"])?;
            enroll_service_identity_at::<IdunnServiceIdentity>(&path(&options, "private-store")?)?;
        }
        "export-idunn-public-anchor" => {
            require_only(&options, &["private-store", "public-anchor"])?;
            let private = path(&options, "private-store")?;
            let public = path(&options, "public-anchor")?;
            reject_alias(&private, &public)?;
            refuse_existing(&public, "public anchor")?;
            let signer = open_service_identity_at::<IdunnServiceIdentity>(&private)?;
            export_service_identity_trust_anchor(&signer, &public)?;
        }
        "create-daemon-health-trust-binding" => create_health_binding(&options)?,
        "add-daemon-health-trust-binding" => add_health_binding(&options)?,
        "validate-daemon-health-trust-binding" => {
            require_only(&options, &["input"])?;
            validate_health_binding_store(&path(&options, "input")?)?;
        }
        "create-provider-projection-trust-anchor" => create_projection_anchor(&options)?,
        "validate-provider-projection-trust-anchor" => validate_projection_anchor(&options)?,
        _ => bail!("unknown command {command:?}\n{}", usage()),
    }
    Ok(())
}

fn validate_health_binding_store(path: &Path) -> Result<()> {
    let entries = SingleFileMessagePackBackingStore::new(path).pull_all_read_only_snapshot()?;
    if entries.is_empty() {
        bail!("daemon health trust store is empty");
    }
    let mut keys = BTreeSet::new();
    let mut tuples = BTreeSet::new();
    for envelope in entries {
        if envelope.r#type != IdunnDaemonHealthTrustBindingRecord::TYPE
            || envelope.schema_id.as_deref() != Some(IDUNN_DAEMON_HEALTH_TRUST_BINDING_SCHEMA)
        {
            bail!("trust store contains an alien type or schema");
        }
        let binding: IdunnDaemonHealthTrustBindingRecord =
            rmp_serde::from_slice(&envelope.payload)?;
        if rmp_serde::to_vec(&binding)? != envelope.payload || envelope.key != binding.binding_id {
            bail!("trust store contains a noncanonical or mismatched binding");
        }
        binding.validate()?;
        if !keys.insert(binding.binding_id.clone())
            || !tuples.insert((
                binding.daemon_id,
                binding.health_contract,
                binding.source_runtime_id,
            ))
        {
            bail!("trust store contains a duplicate binding id or tuple");
        }
    }
    Ok(())
}

fn create_health_binding(options: &BTreeMap<String, String>) -> Result<()> {
    let record = health_binding(options)?;
    write_new_typed(
        &path(options, "output")?,
        &record.binding_id,
        &record,
        IDUNN_DAEMON_HEALTH_TRUST_BINDING_SCHEMA,
        record.bound_at_unix_millis,
    )
}

fn health_binding(
    options: &BTreeMap<String, String>,
) -> Result<IdunnDaemonHealthTrustBindingRecord> {
    require_only(
        options,
        &[
            "output",
            "binding-id",
            "daemon",
            "health-contract",
            "source-runtime",
            "signer-public-key-hex",
            "bound-at-unix-millis",
            "release-binding-required",
        ],
    )?;
    let public_key = decode_public_key(required(options, "signer-public-key-hex")?)?;
    let record = IdunnDaemonHealthTrustBindingRecord {
        schema_version: IDUNN_DAEMON_HEALTH_TRUST_BINDING_SCHEMA.into(),
        binding_id: required(options, "binding-id")?.into(),
        daemon_id: required(options, "daemon")?.into(),
        health_contract: required(options, "health-contract")?.into(),
        source_runtime_id: required(options, "source-runtime")?.into(),
        signer_identity_id: derive_service_identity_id::<GameCultProviderHealthIdentity>(
            &public_key,
        )?,
        signer_public_key: public_key,
        binding_authority: "root".into(),
        bound_at_unix_millis: parse_u64(options, "bound-at-unix-millis")?,
        release_binding_required: parse_bool(options, "release-binding-required")?,
        private_state_exposed: false,
    };
    record.validate()?;
    Ok(record)
}

fn add_health_binding(options: &BTreeMap<String, String>) -> Result<()> {
    let record = health_binding(options)?;
    let output = path(options, "output")?;
    let store = SingleFileMessagePackBackingStore::new(&output);
    let existing = store.pull_all_read_only_snapshot()?;
    if existing.is_empty() {
        bail!("trust store must already exist; create the first binding explicitly");
    }
    let mut keys = BTreeSet::new();
    let mut tuples = BTreeSet::new();
    let mut expected = Vec::with_capacity(existing.len() + 1);
    for envelope in existing {
        if envelope.r#type != IdunnDaemonHealthTrustBindingRecord::TYPE
            || envelope.schema_id.as_deref() != Some(IDUNN_DAEMON_HEALTH_TRUST_BINDING_SCHEMA)
        {
            bail!("trust store contains an alien type or schema");
        }
        let binding: IdunnDaemonHealthTrustBindingRecord =
            rmp_serde::from_slice(&envelope.payload)?;
        if rmp_serde::to_vec(&binding)? != envelope.payload || envelope.key != binding.binding_id {
            bail!("trust store contains a noncanonical or mismatched binding");
        }
        binding.validate()?;
        if !keys.insert(binding.binding_id.clone()) {
            bail!("trust store contains duplicate binding ids");
        }
        if !tuples.insert((
            binding.daemon_id,
            binding.health_contract,
            binding.source_runtime_id,
        )) {
            bail!("trust store contains duplicate daemon/contract/runtime tuples");
        }
        expected.push(CultCacheExpectedEnvelope {
            r#type: envelope.r#type.clone(),
            key: envelope.key.clone(),
            current: Some(envelope),
        });
    }
    if keys.contains(&record.binding_id) {
        bail!("binding id already exists");
    }
    if tuples.contains(&(
        record.daemon_id.clone(),
        record.health_contract.clone(),
        record.source_runtime_id.clone(),
    )) {
        bail!("daemon/contract/runtime tuple already exists");
    }
    expected.push(CultCacheExpectedEnvelope {
        r#type: IdunnDaemonHealthTrustBindingRecord::TYPE.into(),
        key: record.binding_id.clone(),
        current: None,
    });
    let envelope = typed_envelope(
        &record.binding_id,
        &record,
        IDUNN_DAEMON_HEALTH_TRUST_BINDING_SCHEMA,
        record.bound_at_unix_millis,
    )?;
    if !store.compare_exchange(&expected, &[envelope])? {
        bail!("trust store changed during validated append");
    }
    Ok(())
}

fn typed_envelope<T: DatabaseEntry>(
    key: &str,
    value: &T,
    schema: &str,
    millis: u64,
) -> Result<CultCacheEnvelope> {
    let stored_at = chrono::DateTime::from_timestamp_millis(i64::try_from(millis)?)
        .ok_or_else(|| anyhow!("document timestamp is out of range"))?
        .to_rfc3339();
    Ok(CultCacheEnvelope {
        key: key.into(),
        r#type: T::TYPE.into(),
        payload: rmp_serde::to_vec(value)?,
        stored_at,
        schema_id: Some(schema.into()),
    })
}

fn create_projection_anchor(options: &BTreeMap<String, String>) -> Result<()> {
    require_only(
        options,
        &[
            "output",
            "trust-anchor-id",
            "runtime-id",
            "idunn-public-anchor",
            "bound-at-unix-millis",
            "expires-at-unix-millis",
        ],
    )?;
    let output = path(options, "output")?;
    let low_path = path(options, "idunn-public-anchor")?;
    reject_alias(&output, &low_path)?;
    let low = read_low_level_idunn_anchor(&low_path)?;
    let bound = parse_u64(options, "bound-at-unix-millis")?;
    let record = GameCultServiceTrustAnchorRecord {
        schema_version: GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA.into(),
        trust_anchor_id: required(options, "trust-anchor-id")?.into(),
        service_id: "idunn".into(),
        runtime_id: required(options, "runtime-id")?.into(),
        signer_identity_id: derive_service_identity_id::<IdunnServiceIdentity>(&low.public_key)?,
        signer_public_key: low.public_key,
        signature_algorithm: "ed25519".into(),
        signing_purpose: IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SIGNING_PURPOSE.into(),
        signed_schema: IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SCHEMA.into(),
        binding_authority: "root".into(),
        bound_at_unix_millis: bound,
        expires_at_unix_millis: options
            .get("expires-at-unix-millis")
            .map(|value| value.parse().context("expires-at-unix-millis must be u64"))
            .transpose()?,
        private_state_exposed: false,
    };
    record.validate()?;
    write_new_typed(
        &output,
        &record.trust_anchor_id,
        &record,
        GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA,
        bound,
    )
}

fn validate_projection_anchor(options: &BTreeMap<String, String>) -> Result<()> {
    require_only(options, &["input", "idunn-public-anchor"])?;
    let input = path(options, "input")?;
    let low_path = path(options, "idunn-public-anchor")?;
    reject_alias(&input, &low_path)?;
    let record = read_typed::<GameCultServiceTrustAnchorRecord>(
        &input,
        GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA,
    )?;
    record.validate()?;
    let low = read_low_level_idunn_anchor(&low_path)?;
    if record.signer_public_key != low.public_key || record.signer_identity_id != low.identity_id {
        bail!("projection trust anchor does not match the supplied Idunn public anchor");
    }
    Ok(())
}

fn write_new_typed<T: RootDistributionDocument>(
    path: &Path,
    key: &str,
    value: &T,
    schema: &str,
    millis: u64,
) -> Result<()> {
    refuse_existing(path, "root distribution document")?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let stored_at = chrono::DateTime::from_timestamp_millis(
        i64::try_from(millis).context("document timestamp is out of range")?,
    )
    .ok_or_else(|| anyhow!("document timestamp is out of range"))?
    .to_rfc3339();
    let envelope = CultCacheEnvelope {
        key: key.into(),
        r#type: T::TYPE.into(),
        payload: rmp_serde::to_vec(value)?,
        stored_at,
        schema_id: Some(schema.into()),
    };
    let created = SingleFileMessagePackBackingStore::new(path).compare_exchange(
        &[CultCacheExpectedEnvelope {
            r#type: T::TYPE.into(),
            key: key.into(),
            current: None,
        }],
        &[envelope],
    )?;
    if !created {
        bail!("root distribution document lost atomic create race; refusing replacement");
    }
    Ok(())
}

fn read_typed<T: RootDistributionDocument>(path: &Path, schema: &str) -> Result<T> {
    let entries = SingleFileMessagePackBackingStore::new(path).pull_all_read_only_snapshot()?;
    let [entry] = entries.as_slice() else {
        bail!("root distribution file must contain exactly one document");
    };
    if entry.r#type != T::TYPE || entry.schema_id.as_deref() != Some(schema) {
        bail!("root distribution document has the wrong type or schema");
    }
    let value: T = rmp_serde::from_slice(&entry.payload).context("decoding typed root document")?;
    if rmp_serde::to_vec(&value)? != entry.payload {
        bail!("root distribution document is not canonical positional MessagePack");
    }
    if entry.key != value.root_key() {
        bail!("root distribution envelope key does not match its typed document");
    }
    Ok(value)
}

fn read_low_level_idunn_anchor(path: &Path) -> Result<ServiceIdentityTrustAnchor> {
    let entries = SingleFileMessagePackBackingStore::new(path).pull_all_read_only_snapshot()?;
    let [entry] = entries.as_slice() else {
        bail!("Idunn public anchor store must contain exactly one document");
    };
    if entry.r#type != IdunnServiceIdentity::TRUST_ANCHOR_TYPE
        || entry.key != IdunnServiceIdentity::TRUST_ANCHOR_KEY
        || entry.schema_id.as_deref() != Some(IdunnServiceIdentity::TRUST_ANCHOR_SCHEMA)
    {
        bail!("public anchor does not belong to the Idunn service identity profile");
    }
    let anchor: ServiceIdentityTrustAnchor = rmp_serde::from_slice(&entry.payload)?;
    if rmp_serde::to_vec(&anchor)? != entry.payload
        || anchor.schema_version != IdunnServiceIdentity::TRUST_ANCHOR_SCHEMA
        || anchor.identity_id
            != derive_service_identity_id::<IdunnServiceIdentity>(&anchor.public_key)?
    {
        bail!("Idunn public anchor is malformed or names the wrong key");
    }
    Ok(anchor)
}

fn parse_options(args: impl Iterator<Item = String>) -> Result<BTreeMap<String, String>> {
    let mut args = args.peekable();
    let mut out = BTreeMap::new();
    while let Some(name) = args.next() {
        let name = name
            .strip_prefix("--")
            .ok_or_else(|| anyhow!("expected --option, got {name:?}"))?;
        if name.contains("seed") || name.contains("private-key") {
            bail!("private seed/key input is forbidden; enroll a protected identity instead");
        }
        let value = args
            .next()
            .ok_or_else(|| anyhow!("missing value for --{name}"))?;
        if out.insert(name.into(), value).is_some() {
            bail!("duplicate option --{name}");
        }
    }
    Ok(out)
}

fn require_only(options: &BTreeMap<String, String>, names: &[&str]) -> Result<()> {
    for name in names {
        required(options, name)?;
    }
    if let Some(name) = options.keys().find(|name| !names.contains(&name.as_str())) {
        bail!("unsupported option --{name}");
    }
    Ok(())
}

fn required<'a>(options: &'a BTreeMap<String, String>, name: &str) -> Result<&'a str> {
    options
        .get(name)
        .map(String::as_str)
        .ok_or_else(|| anyhow!("missing --{name}"))
}

fn path(options: &BTreeMap<String, String>, name: &str) -> Result<PathBuf> {
    let value = required(options, name)?;
    if value.trim().is_empty() {
        bail!("--{name} must not be empty");
    }
    Ok(PathBuf::from(value))
}

fn parse_u64(options: &BTreeMap<String, String>, name: &str) -> Result<u64> {
    required(options, name)?
        .parse()
        .with_context(|| format!("--{name} must be u64"))
}

fn parse_bool(options: &BTreeMap<String, String>, name: &str) -> Result<bool> {
    match required(options, name)? {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => bail!("--{name} must be true or false"),
    }
}

fn decode_public_key(value: &str) -> Result<Vec<u8>> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
    {
        bail!("signer public key must be exactly 64 lowercase hexadecimal characters");
    }
    (0..32)
        .map(|index| u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).map_err(Into::into))
        .collect()
}

fn refuse_existing(path: &Path, label: &str) -> Result<()> {
    if path.exists() {
        bail!(
            "{label} {} already exists; replacement is forbidden",
            path.display()
        );
    }
    Ok(())
}

fn reject_alias(first: &Path, second: &Path) -> Result<()> {
    if normalized(first)? == normalized(second)? {
        bail!("private/public or input/output paths must not alias");
    }
    Ok(())
}

fn normalized(path: &Path) -> Result<PathBuf> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let parent = absolute
        .parent()
        .ok_or_else(|| anyhow!("path has no parent"))?;
    let parent = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    Ok(parent.join(
        absolute
            .file_name()
            .ok_or_else(|| anyhow!("path has no filename"))?,
    ))
}

fn usage() -> &'static str {
    "Usage: idunn-provision enroll-idunn-identity --private-store <path>\n       idunn-provision export-idunn-public-anchor --private-store <path> --public-anchor <path>\n       idunn-provision create-daemon-health-trust-binding --output <path> --binding-id <id> --daemon <id> --health-contract <id> --source-runtime <id> --signer-public-key-hex <hex> --bound-at-unix-millis <u64> --release-binding-required <true|false>\n       idunn-provision add-daemon-health-trust-binding --output <path> --binding-id <id> --daemon <id> --health-contract <id> --source-runtime <id> --signer-public-key-hex <hex> --bound-at-unix-millis <u64> --release-binding-required <true|false>\n       idunn-provision validate-daemon-health-trust-binding --input <path>\n       idunn-provision create-provider-projection-trust-anchor --output <path> --trust-anchor-id <id> --runtime-id <id> --idunn-public-anchor <path> --bound-at-unix-millis <u64> --expires-at-unix-millis <u64>\n       idunn-provision validate-provider-projection-trust-anchor --input <path> --idunn-public-anchor <path>"
}

#[cfg(test)]
mod tests {
    use super::*;
    use cultcache_rs::CacheBackingStore;
    use tempfile::TempDir;

    fn invoke(args: &[&str]) -> Result<()> {
        run(args.iter().map(|value| value.to_string()))
    }

    fn replace_typed<T: DatabaseEntry>(
        path: &Path,
        key: &str,
        schema: &str,
        value: &T,
    ) -> Result<()> {
        std::fs::remove_file(path)?;
        SingleFileMessagePackBackingStore::new(path).push(&CultCacheEnvelope {
            key: key.into(),
            r#type: T::TYPE.into(),
            payload: rmp_serde::to_vec(value)?,
            stored_at: "2026-07-19T20:00:00Z".into(),
            schema_id: Some(schema.into()),
        })
    }

    fn binding_args<'a>(
        command: &'a str,
        output: &'a str,
        id: &'a str,
        daemon: &'a str,
        runtime: &'a str,
    ) -> Vec<&'a str> {
        vec![
            command,
            "--output",
            output,
            "--binding-id",
            id,
            "--daemon",
            daemon,
            "--health-contract",
            "provider.health",
            "--source-runtime",
            runtime,
            "--signer-public-key-hex",
            "1111111111111111111111111111111111111111111111111111111111111111",
            "--bound-at-unix-millis",
            "1784483100000",
            "--release-binding-required",
            "false",
        ]
    }

    #[test]
    fn append_health_binding_preserves_existing_and_rejects_collisions_and_aliens() -> Result<()> {
        let temp = TempDir::new()?;
        let store = temp.path().join("trust.cc");
        let path = store.to_str().unwrap();
        invoke(&binding_args(
            "create-daemon-health-trust-binding",
            path,
            "one",
            "daemon-one",
            "runtime-one",
        ))?;
        let before = SingleFileMessagePackBackingStore::new(&store)
            .pull_all_read_only_snapshot()?
            .remove(0)
            .payload;
        invoke(&binding_args(
            "add-daemon-health-trust-binding",
            path,
            "two",
            "daemon-two",
            "runtime-two",
        ))?;
        let entries =
            SingleFileMessagePackBackingStore::new(&store).pull_all_read_only_snapshot()?;
        assert_eq!(entries.len(), 2);
        assert_eq!(
            entries
                .iter()
                .find(|entry| entry.key == "one")
                .unwrap()
                .payload,
            before
        );
        assert!(
            invoke(&binding_args(
                "add-daemon-health-trust-binding",
                path,
                "two",
                "daemon-three",
                "runtime-three"
            ))
            .is_err()
        );
        assert!(
            invoke(&binding_args(
                "add-daemon-health-trust-binding",
                path,
                "three",
                "daemon-one",
                "runtime-one"
            ))
            .is_err()
        );
        SingleFileMessagePackBackingStore::new(&store).push(&CultCacheEnvelope {
            key: "alien".into(),
            r#type: "alien".into(),
            payload: vec![],
            stored_at: "2026-07-19T20:00:00Z".into(),
            schema_id: Some("alien.v0".into()),
        })?;
        assert!(
            invoke(&binding_args(
                "add-daemon-health-trust-binding",
                path,
                "four",
                "daemon-four",
                "runtime-four"
            ))
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn concurrent_health_binding_appends_have_one_cas_winner() -> Result<()> {
        let temp = TempDir::new()?;
        let store = temp.path().join("trust.cc");
        let path = store.to_str().unwrap().to_string();
        invoke(&binding_args("create-daemon-health-trust-binding", &path, "one", "daemon-one", "runtime-one"))?;
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
        let workers = [("two", "daemon-two", "runtime-two"), ("three", "daemon-three", "runtime-three")].map(|(id, daemon, runtime)| {
            let path = path.clone(); let barrier = barrier.clone();
            std::thread::spawn(move || { barrier.wait(); invoke(&binding_args("add-daemon-health-trust-binding", &path, id, daemon, runtime)) })
        });
        barrier.wait();
        let wins = workers.into_iter().map(|worker| worker.join().unwrap()).filter(Result::is_ok).count();
        assert!((1..=2).contains(&wins));
        assert_eq!(SingleFileMessagePackBackingStore::new(&store).pull_all_read_only_snapshot()?.len(), 1 + wins);
        Ok(())
    }

    #[test]
    fn identity_enrollment_and_export_are_immutable_and_paths_cannot_alias() -> Result<()> {
        let temp = TempDir::new()?;
        let private = temp.path().join("identity.cc");
        let public = temp.path().join("identity-public.cc");
        invoke(&[
            "enroll-idunn-identity",
            "--private-store",
            private.to_str().unwrap(),
        ])?;
        assert!(
            invoke(&[
                "enroll-idunn-identity",
                "--private-store",
                private.to_str().unwrap()
            ])
            .is_err()
        );
        assert!(
            invoke(&[
                "export-idunn-public-anchor",
                "--private-store",
                private.to_str().unwrap(),
                "--public-anchor",
                private.to_str().unwrap(),
            ])
            .is_err()
        );
        invoke(&[
            "export-idunn-public-anchor",
            "--private-store",
            private.to_str().unwrap(),
            "--public-anchor",
            public.to_str().unwrap(),
        ])?;
        assert!(
            invoke(&[
                "export-idunn-public-anchor",
                "--private-store",
                private.to_str().unwrap(),
                "--public-anchor",
                public.to_str().unwrap(),
            ])
            .is_err()
        );
        assert!(
            invoke(&[
                "enroll-idunn-identity",
                "--private-store",
                temp.path().join("new.cc").to_str().unwrap(),
                "--private-seed",
                "00",
            ])
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn health_binding_derives_identity_and_rejects_overwrite_and_corruption() -> Result<()> {
        let temp = TempDir::new()?;
        let output = temp.path().join("provider-binding.cc");
        let key = "07".repeat(32);
        let args = [
            "create-daemon-health-trust-binding",
            "--output",
            output.to_str().unwrap(),
            "--binding-id",
            "root/epiphany/health",
            "--daemon",
            "yggdrasil-epiphany",
            "--health-contract",
            "epiphany.cultnet-rudp-runtime-health",
            "--source-runtime",
            "epiphany-daemon-supervisor",
            "--signer-public-key-hex",
            &key,
            "--bound-at-unix-millis",
            "1784483100000",
            "--release-binding-required",
            "true",
        ];
        invoke(&args)?;
        assert!(invoke(&args).is_err());
        let mut record = read_typed::<IdunnDaemonHealthTrustBindingRecord>(
            &output,
            IDUNN_DAEMON_HEALTH_TRUST_BINDING_SCHEMA,
        )?;
        assert_eq!(
            record.signer_identity_id,
            derive_service_identity_id::<GameCultProviderHealthIdentity>(
                &record.signer_public_key
            )?
        );
        record.signer_identity_id = "caller-chosen".into();
        replace_typed(
            &output,
            &record.binding_id,
            IDUNN_DAEMON_HEALTH_TRUST_BINDING_SCHEMA,
            &record,
        )?;
        assert!(
            invoke(&[
                "validate-daemon-health-trust-binding",
                "--input",
                output.to_str().unwrap()
            ])
            .is_err()
        );

        record.signer_identity_id = derive_service_identity_id::<GameCultProviderHealthIdentity>(
            &record.signer_public_key,
        )?;
        record.schema_version = "wrong.schema.v9".into();
        replace_typed(
            &output,
            &record.binding_id,
            IDUNN_DAEMON_HEALTH_TRUST_BINDING_SCHEMA,
            &record,
        )?;
        assert!(
            invoke(&[
                "validate-daemon-health-trust-binding",
                "--input",
                output.to_str().unwrap()
            ])
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn projection_anchor_is_exact_profile_and_matches_low_level_anchor() -> Result<()> {
        let temp = TempDir::new()?;
        let private = temp.path().join("identity.cc");
        let public = temp.path().join("public.cc");
        let root = temp.path().join("root-anchor.cc");
        invoke(&[
            "enroll-idunn-identity",
            "--private-store",
            private.to_str().unwrap(),
        ])?;
        invoke(&[
            "export-idunn-public-anchor",
            "--private-store",
            private.to_str().unwrap(),
            "--public-anchor",
            public.to_str().unwrap(),
        ])?;
        let create = [
            "create-provider-projection-trust-anchor",
            "--output",
            root.to_str().unwrap(),
            "--trust-anchor-id",
            "root/idunn/provider-health",
            "--runtime-id",
            "idunn-yggdrasil",
            "--idunn-public-anchor",
            public.to_str().unwrap(),
            "--bound-at-unix-millis",
            "1784483100000",
            "--expires-at-unix-millis",
            "1815000000000",
        ];
        invoke(&create)?;
        invoke(&[
            "validate-provider-projection-trust-anchor",
            "--input",
            root.to_str().unwrap(),
            "--idunn-public-anchor",
            public.to_str().unwrap(),
        ])?;
        assert!(invoke(&create).is_err());
        assert!(
            invoke(&[
                "validate-provider-projection-trust-anchor",
                "--input",
                public.to_str().unwrap(),
                "--idunn-public-anchor",
                public.to_str().unwrap(),
            ])
            .is_err()
        );

        let mut record = read_typed::<GameCultServiceTrustAnchorRecord>(
            &root,
            GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA,
        )?;
        record.signing_purpose = "wrong-purpose".into();
        replace_typed(
            &root,
            &record.trust_anchor_id,
            GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA,
            &record,
        )?;
        assert!(
            invoke(&[
                "validate-provider-projection-trust-anchor",
                "--input",
                root.to_str().unwrap(),
                "--idunn-public-anchor",
                public.to_str().unwrap(),
            ])
            .is_err()
        );

        record.signing_purpose =
            IDUNN_AUTHENTICATED_PROVIDER_HEALTH_PROJECTION_SIGNING_PURPOSE.into();
        record.signer_public_key = vec![9; 32];
        replace_typed(
            &root,
            &record.trust_anchor_id,
            GAMECULT_SERVICE_TRUST_ANCHOR_SCHEMA,
            &record,
        )?;
        assert!(
            invoke(&[
                "validate-provider-projection-trust-anchor",
                "--input",
                root.to_str().unwrap(),
                "--idunn-public-anchor",
                public.to_str().unwrap(),
            ])
            .is_err()
        );
        Ok(())
    }

    #[test]
    fn release_binding_policy_is_explicit_and_boolean() {
        let temp = TempDir::new().unwrap();
        let output = temp.path().join("binding.cc");
        let base = [
            "create-daemon-health-trust-binding",
            "--output",
            output.to_str().unwrap(),
            "--binding-id",
            "root/provider/health",
            "--daemon",
            "provider",
            "--health-contract",
            "provider.health",
            "--source-runtime",
            "provider-runtime",
            "--signer-public-key-hex",
            "0707070707070707070707070707070707070707070707070707070707070707",
            "--bound-at-unix-millis",
            "1784483100000",
        ];
        assert!(invoke(&base).is_err());
        let mut partial = base.to_vec();
        partial.extend(["--release-binding-required", "sometimes"]);
        assert!(invoke(&partial).is_err());
    }
}

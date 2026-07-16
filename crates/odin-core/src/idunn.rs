use crate::documents::{
    IdunnDaemonHealthRecord, IdunnDeploymentRequestRecord, IdunnDesiredDaemonRecord,
    IdunnKeepaliveDecisionRecord, IdunnOperatorAlarmRecord, IdunnRestartRequestRecord,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IdunnPlan {
    pub decision: IdunnKeepaliveDecisionRecord,
    pub deployment_request: Option<IdunnDeploymentRequestRecord>,
    pub restart_request: Option<IdunnRestartRequestRecord>,
    pub operator_alarm: Option<IdunnOperatorAlarmRecord>,
}

pub fn plan_keepalive(
    desired: &IdunnDesiredDaemonRecord,
    health: &IdunnDaemonHealthRecord,
    now: impl Into<String>,
) -> IdunnPlan {
    let now = now.into();
    let decision_id = format!("{}:{}", desired.daemon_id, now);

    if !desired.enabled {
        return IdunnPlan {
            decision: decision(&decision_id, desired, "noop", "daemon is disabled", &now),
            deployment_request: None,
            restart_request: None,
            operator_alarm: None,
        };
    }

    if is_healthy(&health.state) {
        return IdunnPlan {
            decision: decision(&decision_id, desired, "observe", "daemon is healthy", &now),
            deployment_request: None,
            restart_request: None,
            operator_alarm: None,
        };
    }

    if health.state == "degraded" || health.state == "dependency-unavailable" {
        let alarm_id = format!("alarm:{}:{}", desired.daemon_id, now);
        let reason = format!(
            "health is {}; local restart/deploy is not the owner of this failure: {}",
            health.state, health.detail
        );
        return IdunnPlan {
            decision: decision(&decision_id, desired, "alarm", &reason, &now),
            deployment_request: None,
            restart_request: None,
            operator_alarm: Some(IdunnOperatorAlarmRecord {
                alarm_id,
                daemon_id: desired.daemon_id.clone(),
                severity: "operator-action-required".to_string(),
                reason,
                escalation_target: "bifrost.operator-notification".to_string(),
                raised_at: now,
            }),
        };
    }

    if let Some(command) = desired.deploy_command.as_deref() {
        if !command.trim().is_empty() && health.state == "stale-deployment" {
            let request_id = format!("deploy:{}:{}", desired.daemon_id, now);
            return IdunnPlan {
                decision: decision(
                    &decision_id,
                    desired,
                    "deploy",
                    &format!(
                        "health is {}; deployment authority is available",
                        health.state
                    ),
                    &now,
                ),
                deployment_request: Some(IdunnDeploymentRequestRecord {
                    request_id,
                    daemon_id: desired.daemon_id.clone(),
                    command: command.to_string(),
                    authority: desired.authority.clone(),
                    requested_at: now,
                    repository_full_name: String::new(),
                    upstream_ref: String::new(),
                    source_revision: String::new(),
                    release_authority_id: String::new(),
                    release_authority_envelope_sha256: String::new(),
                    requires_bifrost_authority: false,
                }),
                restart_request: None,
                operator_alarm: None,
            };
        }
    }

    if health.state == "stale-deployment" {
        let alarm_id = format!("alarm:{}:{}", desired.daemon_id, now);
        let reason = format!(
            "health is stale-deployment under {}; no deploy command authority is available",
            desired.health_contract
        );
        return IdunnPlan {
            decision: decision(&decision_id, desired, "alarm", &reason, &now),
            deployment_request: None,
            restart_request: None,
            operator_alarm: Some(IdunnOperatorAlarmRecord {
                alarm_id,
                daemon_id: desired.daemon_id.clone(),
                severity: "operator-action-required".to_string(),
                reason,
                escalation_target: "bifrost.operator-notification".to_string(),
                raised_at: now,
            }),
        };
    }

    match desired.restart_command.as_deref() {
        Some(command) if !command.trim().is_empty() => {
            let request_id = format!("restart:{}:{}", desired.daemon_id, now);
            IdunnPlan {
                decision: decision(
                    &decision_id,
                    desired,
                    "restart",
                    &format!("health is {}; restart authority is available", health.state),
                    &now,
                ),
                restart_request: Some(IdunnRestartRequestRecord {
                    request_id,
                    daemon_id: desired.daemon_id.clone(),
                    command: command.to_string(),
                    authority: desired.authority.clone(),
                    requested_at: now,
                }),
                deployment_request: None,
                operator_alarm: None,
            }
        }
        _ => {
            let alarm_id = format!("alarm:{}:{}", desired.daemon_id, now);
            let reason = format!(
                "health is {}; no restart command authority is available",
                health.state
            );
            IdunnPlan {
                decision: decision(&decision_id, desired, "alarm", &reason, &now),
                deployment_request: None,
                restart_request: None,
                operator_alarm: Some(IdunnOperatorAlarmRecord {
                    alarm_id,
                    daemon_id: desired.daemon_id.clone(),
                    severity: "operator-action-required".to_string(),
                    reason,
                    escalation_target: "bifrost.operator-notification".to_string(),
                    raised_at: now,
                }),
            }
        }
    }
}

fn is_healthy(state: &str) -> bool {
    matches!(state, "active" | "healthy" | "ok" | "running")
}

fn decision(
    decision_id: &str,
    desired: &IdunnDesiredDaemonRecord,
    action: &str,
    reason: &str,
    decided_at: &str,
) -> IdunnKeepaliveDecisionRecord {
    IdunnKeepaliveDecisionRecord {
        decision_id: decision_id.to_string(),
        daemon_id: desired.daemon_id.clone(),
        action: action.to_string(),
        reason: reason.to_string(),
        authority: desired.authority.clone(),
        decided_at: decided_at.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::documents::{
        IDUNN_COMMAND_BOUNDARY_SCHEMA, IDUNN_DAEMON_SURGERY_PLAN_SCHEMA,
        IDUNN_DAEMON_TRANSPORT_PROFILE_SCHEMA, IDUNN_DEPLOYMENT_ARTIFACT_SCHEMA,
        IDUNN_DESIRED_DAEMON_SCHEMA, IDUNN_RELEASE_TARGET_SCHEMA, IDUNN_ROLLOUT_PLAN_SCHEMA,
        IDUNN_STATE_MIGRATION_PLAN_SCHEMA, IDUNN_SWARM_SURGERY_PLAN_SCHEMA,
        IdunnCommandBoundaryRecord, IdunnDaemonSurgeryPlanRecord,
        IdunnDaemonTransportProfileRecord, IdunnDeploymentArtifactRecord, IdunnReleaseTargetRecord,
        IdunnRolloutPlanRecord, IdunnRuntimeTransportCheckRecord, IdunnStateMigrationPlanRecord,
        IdunnSwarmSurgeryPlanRecord, OdinDocuments,
    };
    use anyhow::Result;
    use cultmesh_rs::{CultMesh, CultMeshNodeOptions};

    fn desired(restart_command: Option<&str>) -> IdunnDesiredDaemonRecord {
        IdunnDesiredDaemonRecord {
            daemon_id: "voidbot".to_string(),
            verse_id: "local".to_string(),
            name: "VoidBot".to_string(),
            enabled: true,
            health_command: None,
            restart_command: restart_command.map(ToString::to_string),
            authority: "idunn-supervisor-command".to_string(),
            max_silence_seconds: 60,
            observed_at: "2026-06-04T00:00:00Z".to_string(),
            deploy_command: None,
            health_contract: "test.cultnet-rudp-health".to_string(),
            transport_profile_id: "transport:voidbot".to_string(),
            command_boundary_id: "command-boundary:voidbot".to_string(),
        }
    }

    fn health(state: &str) -> IdunnDaemonHealthRecord {
        IdunnDaemonHealthRecord {
            daemon_id: "voidbot".to_string(),
            state: state.to_string(),
            detail: "unit daemon publication".to_string(),
            observed_at: "2026-06-04T00:00:01Z".to_string(),
            health_contract: "test.cultnet-rudp-health".to_string(),
            publication_source: "debug-command".to_string(),
            transport: "debug.local-command".to_string(),
        }
    }

    #[test]
    fn healthy_daemon_is_observed_without_restart() {
        let plan = plan_keepalive(
            &desired(Some("npm start")),
            &health("active"),
            "2026-06-04T00:00:02Z",
        );

        assert_eq!(plan.decision.action, "observe");
        assert!(plan.deployment_request.is_none());
        assert!(plan.restart_request.is_none());
        assert!(plan.operator_alarm.is_none());
    }

    #[test]
    fn unhealthy_daemon_with_restart_authority_requests_restart() {
        let plan = plan_keepalive(
            &desired(Some("npm start")),
            &health("failed"),
            "2026-06-04T00:00:02Z",
        );

        assert_eq!(plan.decision.action, "restart");
        assert!(plan.deployment_request.is_none());
        assert_eq!(plan.restart_request.unwrap().command, "npm start");
        assert!(plan.operator_alarm.is_none());
    }

    #[test]
    fn unhealthy_daemon_with_deploy_authority_requests_deployment_before_restart() {
        let mut desired = desired(Some("systemctl restart gjallar"));
        desired.deploy_command = Some("deploy gjallar".to_string());
        let plan = plan_keepalive(
            &desired,
            &health("stale-deployment"),
            "2026-06-04T00:00:02Z",
        );

        assert_eq!(plan.decision.action, "deploy");
        assert_eq!(plan.deployment_request.unwrap().command, "deploy gjallar");
        assert!(plan.restart_request.is_none());
        assert!(plan.operator_alarm.is_none());
    }

    #[test]
    fn failed_daemon_with_deploy_and_restart_authority_restarts_instead_of_redeploying() {
        let mut desired = desired(Some("systemctl restart gjallar"));
        desired.deploy_command = Some("deploy gjallar".to_string());
        let plan = plan_keepalive(&desired, &health("failed"), "2026-06-04T00:00:02Z");

        assert_eq!(plan.decision.action, "restart");
        assert!(plan.deployment_request.is_none());
        assert_eq!(
            plan.restart_request.unwrap().command,
            "systemctl restart gjallar"
        );
        assert!(plan.operator_alarm.is_none());
    }

    #[test]
    fn dependency_unavailable_raises_alarm_instead_of_redeploying_or_restarting() {
        let mut desired = desired(Some("systemctl restart gjallar"));
        desired.deploy_command = Some("deploy gjallar".to_string());
        let plan = plan_keepalive(
            &desired,
            &health("dependency-unavailable"),
            "2026-06-04T00:00:02Z",
        );

        assert_eq!(plan.decision.action, "alarm");
        assert!(plan.deployment_request.is_none());
        assert!(plan.restart_request.is_none());
        assert!(plan.operator_alarm.is_some());
    }

    #[test]
    fn stale_deployment_without_deploy_authority_raises_alarm_instead_of_restarting() {
        let plan = plan_keepalive(
            &desired(Some("systemctl restart old-app")),
            &health("stale-deployment"),
            "2026-06-04T00:00:02Z",
        );

        assert_eq!(plan.decision.action, "alarm");
        assert!(plan.deployment_request.is_none());
        assert!(plan.restart_request.is_none());
        assert!(plan.operator_alarm.is_some());
    }

    #[test]
    fn unhealthy_daemon_without_authority_raises_operator_alarm() {
        let plan = plan_keepalive(&desired(None), &health("failed"), "2026-06-04T00:00:02Z");

        assert_eq!(plan.decision.action, "alarm");
        assert!(plan.deployment_request.is_none());
        assert!(plan.restart_request.is_none());
        assert_eq!(
            plan.operator_alarm.unwrap().escalation_target,
            "bifrost.operator-notification"
        );
    }

    #[test]
    fn idunn_records_round_trip_through_cultmesh() -> Result<()> {
        let temp = tempfile::tempdir()?;
        let store_path = temp.path().join("idunn.cc");
        let desired = desired(Some("npm start"));
        let health = health("failed");
        let plan = plan_keepalive(&desired, &health, "2026-06-04T00:00:02Z");
        let request = plan.restart_request.unwrap();

        let mut node = CultMesh::create_node(
            &store_path,
            OdinDocuments,
            CultMeshNodeOptions {
                runtime_id: "idunn-test".to_string(),
                pull_on_start: true,
            },
        )?;
        node.put(&desired.daemon_id, &desired)?;
        node.put(&health.daemon_id, &health)?;
        node.put(&plan.decision.decision_id, &plan.decision)?;
        node.put(&request.request_id, &request)?;
        let surgery_plan = IdunnDaemonSurgeryPlanRecord {
            plan_id: "surgery:voidbot".to_string(),
            daemon_id: "voidbot".to_string(),
            severity: "high".to_string(),
            status: "transport-surgery-required".to_string(),
            owner: "VoidBot internal provider stack".to_string(),
            objective: "Publish VoidBot internal daemon health over CultNet/RUDP".to_string(),
            current_mechanism: "daemon-published RUDP health".to_string(),
            intended_authority: "daemon-owned CultNet/RUDP health record".to_string(),
            cut_line: "daemon health must be published over CultNet/RUDP".to_string(),
            steps: vec![
                "update CultLib".to_string(),
                "publish RUDP health".to_string(),
            ],
            blockers: vec!["runtime CultLib update".to_string()],
            updated_at: "2026-06-04T00:00:02Z".to_string(),
        };
        let swarm_surgery_plan = IdunnSwarmSurgeryPlanRecord {
            plan_id: "swarm-surgery:starfire-local".to_string(),
            profile: "starfire-local".to_string(),
            status: "active-transport-migration".to_string(),
            owner: "Idunn swarm supervisor".to_string(),
            objective: "Move daemon awareness to CultNet/RUDP".to_string(),
            current_mechanism: "daemon-published RUDP health required".to_string(),
            invariants: vec!["daemon truth is typed RUDP state".to_string()],
            phases: vec!["install Muninn RUDP health".to_string()],
            current_phase: "Phase 2".to_string(),
            next_target: "starfire-muninn".to_string(),
            cut_line: "daemon health must be published over CultNet/RUDP".to_string(),
            verification_layer: "CultMesh keepalive store".to_string(),
            updated_at: "2026-06-04T00:00:02Z".to_string(),
        };
        node.put(&swarm_surgery_plan.plan_id, &swarm_surgery_plan)?;
        node.put(&surgery_plan.plan_id, &surgery_plan)?;
        let transport_profile = IdunnDaemonTransportProfileRecord {
            profile_id: "transport:voidbot".to_string(),
            daemon_id: "voidbot".to_string(),
            target_transport: "cultnet.transport.rudp.v0".to_string(),
            current_transport: "missing-daemon-published-rudp-health".to_string(),
            state: "migration-required".to_string(),
            health_contract: "test.cultnet-rudp-health".to_string(),
            publication_schema: "idunn.daemon_health.v1".to_string(),
            debug_mechanism: "none".to_string(),
            cut_line: "daemon health must be published over CultNet/RUDP".to_string(),
            observed_at: "2026-06-04T00:00:02Z".to_string(),
        };
        node.put(&transport_profile.profile_id, &transport_profile)?;
        let command_boundary = IdunnCommandBoundaryRecord {
            boundary_id: "command-boundary:voidbot".to_string(),
            daemon_id: "voidbot".to_string(),
            owner: "idunn-supervisor-command-boundary".to_string(),
            restart_authority: "idunn-supervisor-command.restart".to_string(),
            deploy_authority: "none".to_string(),
            health_authority: "daemon-published-rudp-health".to_string(),
            alarm_authority: "bifrost.operator-notification".to_string(),
            command_lowerings: vec!["npm start".to_string()],
            forbidden_authority: "debug probes do not own truth".to_string(),
            observed_at: "2026-06-04T00:00:02Z".to_string(),
        };
        node.put(&command_boundary.boundary_id, &command_boundary)?;
        let release_target = IdunnReleaseTargetRecord {
            target_id: "release-target:voidbot".to_string(),
            daemon_id: "voidbot".to_string(),
            repo: "VoidBot".to_string(),
            repo_path: "E:\\Projects\\VoidBot".to_string(),
            upstream_remote: "origin".to_string(),
            upstream_branch: "main".to_string(),
            desired_revision: "abc123".to_string(),
            deployed_revision: "old123".to_string(),
            artifact_strategy: "source-archive-from-upstream-main".to_string(),
            rollout_strategy: "restart-after-verified-build".to_string(),
            state_migration_authority: "daemon-owned-command".to_string(),
            zero_downtime_capability: "restart-required".to_string(),
            status: "tracked".to_string(),
            observed_at: "2026-06-04T00:00:02Z".to_string(),
            repository_full_name: "GameCult/VoidBot".to_string(),
            upstream_ref: "refs/heads/main".to_string(),
            release_authority_id: "release:GameCult/VoidBot:refs/heads/main:abc123".to_string(),
            release_authority_envelope_sha256: "authority-sha".to_string(),
            release_authority_status: "authorized".to_string(),
            requires_bifrost_authority: true,
            observed_upstream_revision: "abc123".to_string(),
        };
        let artifact = IdunnDeploymentArtifactRecord {
            artifact_id: "artifact:voidbot:main".to_string(),
            daemon_id: "voidbot".to_string(),
            source_revision: "abc123".to_string(),
            source_branch: "main".to_string(),
            source_remote: "origin".to_string(),
            artifact_kind: "source-archive-from-upstream-main".to_string(),
            artifact_uri: "built-by-deploy-command".to_string(),
            sha256: "pending-deploy-command".to_string(),
            built_at: "2026-06-04T00:00:02Z".to_string(),
            release_authority_id: release_target.release_authority_id.clone(),
            release_authority_envelope_sha256: release_target
                .release_authority_envelope_sha256
                .clone(),
        };
        let migration_plan = IdunnStateMigrationPlanRecord {
            plan_id: "migration:voidbot:main".to_string(),
            daemon_id: "voidbot".to_string(),
            from_schema_version: "deployed-state".to_string(),
            to_schema_version: "target-revision-state".to_string(),
            authority: "daemon-owned-migrator".to_string(),
            command: "voidbot migrate-state".to_string(),
            strategy: "backup-then-daemon-migrator".to_string(),
            backup_required: true,
            zero_downtime_required: false,
            status: "planned".to_string(),
            planned_at: "2026-06-04T00:00:02Z".to_string(),
        };
        let rollout_plan = IdunnRolloutPlanRecord {
            plan_id: "rollout:voidbot:main".to_string(),
            daemon_id: "voidbot".to_string(),
            desired_revision: "abc123".to_string(),
            deployed_revision: "old123".to_string(),
            strategy: "restart-after-verified-build".to_string(),
            phases: vec![
                "fetch upstream main".to_string(),
                "verify health".to_string(),
            ],
            migration_plan_id: migration_plan.plan_id.clone(),
            artifact_id: artifact.artifact_id.clone(),
            status: "planned".to_string(),
            planned_at: "2026-06-04T00:00:02Z".to_string(),
        };
        node.put(&release_target.target_id, &release_target)?;
        node.put(&artifact.artifact_id, &artifact)?;
        node.put(&migration_plan.plan_id, &migration_plan)?;
        node.put(&rollout_plan.plan_id, &rollout_plan)?;
        let runtime_check = IdunnRuntimeTransportCheckRecord {
            check_id: "idunn-runtime-rudp-loopback".to_string(),
            runtime_id: "idunn-daemon".to_string(),
            transport: "cultnet.transport.rudp.v0".to_string(),
            state: "available".to_string(),
            detail: "loopback acknowledged".to_string(),
            observed_at: "2026-06-04T00:00:02Z".to_string(),
        };
        node.put(&runtime_check.check_id, &runtime_check)?;

        let reloaded = CultMesh::create_node(
            &store_path,
            OdinDocuments,
            CultMeshNodeOptions {
                runtime_id: "idunn-test-reloaded".to_string(),
                pull_on_start: true,
            },
        )?;

        assert_eq!(
            reloaded
                .documents()
                .binding("idunn.desired_daemon")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(IDUNN_DESIRED_DAEMON_SCHEMA)
        );
        assert_eq!(
            reloaded
                .documents()
                .binding("idunn.swarm_surgery_plan")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(IDUNN_SWARM_SURGERY_PLAN_SCHEMA)
        );
        assert_eq!(
            reloaded
                .documents()
                .binding("idunn.daemon_surgery_plan")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(IDUNN_DAEMON_SURGERY_PLAN_SCHEMA)
        );
        assert_eq!(
            reloaded
                .documents()
                .binding("idunn.daemon_transport_profile")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(IDUNN_DAEMON_TRANSPORT_PROFILE_SCHEMA)
        );
        assert_eq!(
            reloaded
                .documents()
                .binding("idunn.command_boundary")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(IDUNN_COMMAND_BOUNDARY_SCHEMA)
        );
        assert_eq!(
            reloaded
                .documents()
                .binding("idunn.release_target")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(IDUNN_RELEASE_TARGET_SCHEMA)
        );
        assert_eq!(
            reloaded
                .documents()
                .binding("idunn.deployment_artifact")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(IDUNN_DEPLOYMENT_ARTIFACT_SCHEMA)
        );
        assert_eq!(
            reloaded
                .documents()
                .binding("idunn.state_migration_plan")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(IDUNN_STATE_MIGRATION_PLAN_SCHEMA)
        );
        assert_eq!(
            reloaded
                .documents()
                .binding("idunn.rollout_plan")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(IDUNN_ROLLOUT_PLAN_SCHEMA)
        );
        let reloaded_desired =
            reloaded.get_required::<IdunnDesiredDaemonRecord>(&desired.daemon_id)?;
        assert_eq!(reloaded_desired.transport_profile_id, "transport:voidbot");
        assert_eq!(
            reloaded_desired.command_boundary_id,
            "command-boundary:voidbot"
        );
        assert_eq!(
            reloaded
                .get_required::<IdunnKeepaliveDecisionRecord>(&plan.decision.decision_id)?
                .action,
            "restart"
        );
        assert_eq!(
            reloaded
                .get_required::<IdunnSwarmSurgeryPlanRecord>(&swarm_surgery_plan.plan_id)?
                .next_target,
            "starfire-muninn"
        );
        assert_eq!(
            reloaded
                .get_required::<IdunnDaemonSurgeryPlanRecord>(&surgery_plan.plan_id)?
                .cut_line,
            "daemon health must be published over CultNet/RUDP"
        );
        assert_eq!(
            reloaded
                .get_required::<IdunnDaemonTransportProfileRecord>(&transport_profile.profile_id)?
                .target_transport,
            "cultnet.transport.rudp.v0"
        );
        assert_eq!(
            reloaded
                .get_required::<IdunnCommandBoundaryRecord>(&command_boundary.boundary_id)?
                .health_authority,
            "daemon-published-rudp-health"
        );
        assert_eq!(
            reloaded
                .get_required::<IdunnRuntimeTransportCheckRecord>(&runtime_check.check_id)?
                .state,
            "available"
        );
        assert_eq!(
            reloaded
                .get_required::<IdunnReleaseTargetRecord>(&release_target.target_id)?
                .upstream_branch,
            "main"
        );
        assert_eq!(
            reloaded
                .get_required::<IdunnRolloutPlanRecord>(&rollout_plan.plan_id)?
                .migration_plan_id,
            "migration:voidbot:main"
        );
        assert_eq!(
            reloaded
                .get_required::<IdunnRestartRequestRecord>(&request.request_id)?
                .command,
            "npm start"
        );
        Ok(())
    }
}

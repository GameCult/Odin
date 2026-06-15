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
        IDUNN_DAEMON_SURGERY_PLAN_SCHEMA, IDUNN_DESIRED_DAEMON_SCHEMA,
        IdunnDaemonSurgeryPlanRecord, OdinDocuments,
    };
    use anyhow::Result;
    use cultmesh_rs::{CultMesh, CultMeshNodeOptions};

    fn desired(restart_command: Option<&str>) -> IdunnDesiredDaemonRecord {
        IdunnDesiredDaemonRecord {
            daemon_id: "voidbot".to_string(),
            verse_id: "local".to_string(),
            name: "VoidBot".to_string(),
            enabled: true,
            health_command: Some("exit 1".to_string()),
            restart_command: restart_command.map(ToString::to_string),
            authority: "idunn.local-command".to_string(),
            max_silence_seconds: 60,
            observed_at: "2026-06-04T00:00:00Z".to_string(),
            deploy_command: None,
            health_contract: "test.command-exit".to_string(),
        }
    }

    fn health(state: &str) -> IdunnDaemonHealthRecord {
        IdunnDaemonHealthRecord {
            daemon_id: "voidbot".to_string(),
            state: state.to_string(),
            detail: "unit probe".to_string(),
            observed_at: "2026-06-04T00:00:01Z".to_string(),
            health_contract: "test.command-exit".to_string(),
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
            current_mechanism: "compatibility command probe".to_string(),
            intended_authority: "daemon-owned CultNet/RUDP health record".to_string(),
            cut_line: "demote command probe after RUDP health exists".to_string(),
            steps: vec![
                "update CultLib".to_string(),
                "publish RUDP health".to_string(),
            ],
            blockers: vec!["runtime CultLib update".to_string()],
            updated_at: "2026-06-04T00:00:02Z".to_string(),
        };
        node.put(&surgery_plan.plan_id, &surgery_plan)?;

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
                .binding("idunn.daemon_surgery_plan")
                .and_then(|binding| binding.payload_schema_version.clone())
                .as_deref(),
            Some(IDUNN_DAEMON_SURGERY_PLAN_SCHEMA)
        );
        assert_eq!(
            reloaded
                .get_required::<IdunnKeepaliveDecisionRecord>(&plan.decision.decision_id)?
                .action,
            "restart"
        );
        assert_eq!(
            reloaded
                .get_required::<IdunnDaemonSurgeryPlanRecord>(&surgery_plan.plan_id)?
                .cut_line,
            "demote command probe after RUDP health exists"
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

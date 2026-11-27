use dbflux_core::{SshAuthMethod, SshTunnelConfig, SshTunnelProfile};

use crate::bootstrap::StorageRuntime;
use crate::repositories::ssh_tunnel_profiles::{SshTunnelProfileDto, SshTunnelProfileRepository};

/// Loads every SSH tunnel profile that can be used.
///
/// Callers keep working without the profiles, so a storage failure yields an
/// empty list and a row that cannot be used is skipped, but both are logged:
/// a connection that references a skipped tunnel later reports it as not
/// found, and the log is the only place that says why.
pub fn load_ssh_tunnel_profiles(runtime: &StorageRuntime) -> Vec<SshTunnelProfile> {
    let repo = runtime.ssh_tunnels();

    match repo.all() {
        Ok(entries) => entries
            .into_iter()
            .filter_map(|dto| profile_from_dto(&repo, dto))
            .collect(),
        Err(error) => {
            log::error!("Failed to load SSH tunnel profiles: {error}");
            Vec::new()
        }
    }
}

fn profile_from_dto(
    repo: &SshTunnelProfileRepository,
    dto: SshTunnelProfileDto,
) -> Option<SshTunnelProfile> {
    let Ok(id) = uuid::Uuid::parse_str(&dto.id) else {
        log::warn!(
            "Skipping SSH tunnel profile '{}': its id '{}' is not a UUID",
            dto.name,
            dto.id
        );
        return None;
    };
    let Ok(port) = u16::try_from(dto.port) else {
        log::warn!(
            "Skipping SSH tunnel profile '{}' ({id}): port {} is out of range",
            dto.name,
            dto.port
        );
        return None;
    };
    let auth_method = match dto.auth_method.as_str() {
        "key" => SshAuthMethod::PrivateKey {
            key_path: key_path(repo, &dto, &id),
        },
        _ => SshAuthMethod::Password,
    };
    let config = SshTunnelConfig {
        host: dto.host,
        port,
        user: dto.user,
        auth_method,
        ssh_config_host: dto.ssh_config_host,
    };
    Some(SshTunnelProfile {
        id,
        name: dto.name,
        config,
        save_secret: dto.save_secret,
    })
}

/// A key path that cannot be read falls back to the SSH agent and default
/// keys, as a profile without one does.
fn key_path(
    repo: &SshTunnelProfileRepository,
    dto: &SshTunnelProfileDto,
    id: &uuid::Uuid,
) -> Option<std::path::PathBuf> {
    match repo.get_auth(&dto.id) {
        Ok(auth) => auth
            .and_then(|auth| auth.key_path)
            .map(std::path::PathBuf::from),
        Err(error) => {
            log::warn!(
                "SSH tunnel profile '{}' ({id}): failed to read its key path, using the SSH agent \
                 and default keys: {error}",
                dto.name
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unusable_rows_are_skipped_and_the_rest_load() {
        let runtime = StorageRuntime::in_memory().expect("runtime");
        let repo = runtime.ssh_tunnels();
        let good_id = uuid::Uuid::new_v4();
        let good = SshTunnelProfileDto::new(
            good_id,
            "good".to_string(),
            "bastion.example.com".to_string(),
            2222,
            "deploy".to_string(),
        );
        let mut bad_id = SshTunnelProfileDto::new(
            uuid::Uuid::new_v4(),
            "bad-id".to_string(),
            "h".to_string(),
            22,
            "u".to_string(),
        );
        bad_id.id = "not-a-uuid".to_string();
        let bad_port = SshTunnelProfileDto::new(
            uuid::Uuid::new_v4(),
            "bad-port".to_string(),
            "h".to_string(),
            70_000,
            "u".to_string(),
        );
        for dto in [&good, &bad_id, &bad_port] {
            repo.insert(dto, None).expect("insert");
        }

        let loaded = load_ssh_tunnel_profiles(&runtime);

        assert_eq!(loaded.len(), 1, "loaded: {loaded:?}");
        assert_eq!(loaded[0].id, good_id);
        assert_eq!(loaded[0].config.port, 2222);
    }
}

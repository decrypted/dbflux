use dbflux_core::{SshAuthMethod, SshTunnelConfig, SshTunnelProfile};

use crate::bootstrap::StorageRuntime;

pub fn load_ssh_tunnel_profiles(runtime: &StorageRuntime) -> Vec<SshTunnelProfile> {
    let repo = runtime.ssh_tunnels();

    if let Ok(entries) = repo.all() {
        entries
            .into_iter()
            .filter_map(|dto| {
                let id = uuid::Uuid::parse_str(&dto.id).ok()?;
                let auth_method = match dto.auth_method.as_str() {
                    "key" => {
                        if let Ok(Some(auth_dto)) = repo.get_auth(&dto.id) {
                            SshAuthMethod::PrivateKey {
                                key_path: auth_dto.key_path.map(std::path::PathBuf::from),
                            }
                        } else {
                            SshAuthMethod::PrivateKey { key_path: None }
                        }
                    }
                    _ => SshAuthMethod::Password,
                };
                let config = SshTunnelConfig {
                    host: dto.host,
                    port: dto.port as u16,
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
            })
            .collect()
    } else {
        Vec::new()
    }
}

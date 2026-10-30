//! Repository for the driver parameters of connection profiles in dbflux.db
//! (`cfg_connection_profile_driver_params`). Rows keep the order the user
//! listed the parameters in, because a later entry for the same key wins.

use dbflux_core::DriverParam;
use rusqlite::{Connection, params};

use crate::bootstrap::OwnedConnection;
use crate::error::StorageError;

pub struct ConnectionProfileDriverParamsRepository {
    conn: OwnedConnection,
}

impl ConnectionProfileDriverParamsRepository {
    pub fn new(conn: OwnedConnection) -> Self {
        Self { conn }
    }

    fn conn(&self) -> &Connection {
        &self.conn
    }

    /// The parameters of a profile, in their stored order.
    pub fn get_for_profile(&self, profile_id: &str) -> Result<Vec<DriverParam>, StorageError> {
        let mut stmt = self
            .conn()
            .prepare(
                "SELECT param_key, param_value
                 FROM cfg_connection_profile_driver_params
                 WHERE profile_id = ?1
                 ORDER BY position ASC",
            )
            .map_err(sqlite_err)?;

        let rows = stmt
            .query_map([profile_id], |row| {
                Ok(DriverParam::new(
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                ))
            })
            .map_err(sqlite_err)?;

        rows.collect::<Result<Vec<_>, _>>().map_err(sqlite_err)
    }

    /// Replaces every parameter of a profile with `driver_params`, numbering
    /// them in list order.
    pub fn replace_for_profile(
        &self,
        profile_id: &str,
        driver_params: &[DriverParam],
    ) -> Result<(), StorageError> {
        self.conn()
            .execute(
                "DELETE FROM cfg_connection_profile_driver_params WHERE profile_id = ?1",
                [profile_id],
            )
            .map_err(sqlite_err)?;

        for (position, param) in driver_params.iter().enumerate() {
            self.conn()
                .execute(
                    "INSERT INTO cfg_connection_profile_driver_params
                        (profile_id, position, param_key, param_value)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![profile_id, position as i64, param.key, param.value],
                )
                .map_err(sqlite_err)?;
        }

        Ok(())
    }
}

fn sqlite_err(source: rusqlite::Error) -> StorageError {
    StorageError::Sqlite {
        path: "dbflux.db".into(),
        source,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::migrations::MigrationRegistry;
    use crate::repositories::connection_profiles::{
        ConnectionProfileDto, ConnectionProfileRepository,
    };
    use crate::sqlite::open_database;

    #[test]
    fn driver_params_round_trip_in_order() {
        let path = std::env::temp_dir().join(format!(
            "dbflux_driver_params_round_trip_{}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);

        let conn = open_database(&path).expect("should open");
        MigrationRegistry::new()
            .run_all(&conn)
            .expect("migration should run");

        #[allow(clippy::arc_with_non_send_sync)]
        let profiles = ConnectionProfileRepository::new(Arc::new(conn));
        let dto = ConnectionProfileDto::new(uuid::Uuid::new_v4(), "Aurora".to_string());
        profiles.insert(&dto).expect("should insert");
        let repo = profiles.driver_params();

        let listed = vec![
            DriverParam::new("compress", "fast"),
            DriverParam::new("stmt_cache_size", "64"),
            DriverParam::new("compress", "true"),
        ];
        repo.replace_for_profile(&dto.id, &listed)
            .expect("should save");
        assert_eq!(repo.get_for_profile(&dto.id).expect("should load"), listed);

        let replaced = vec![DriverParam::new("compress", "false")];
        repo.replace_for_profile(&dto.id, &replaced)
            .expect("should save");
        assert_eq!(
            repo.get_for_profile(&dto.id).expect("should load"),
            replaced
        );

        repo.replace_for_profile(&dto.id, &[]).expect("should save");
        assert!(
            repo.get_for_profile(&dto.id)
                .expect("should load")
                .is_empty()
        );

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(path.with_extension("sqlite-wal"));
        let _ = std::fs::remove_file(path.with_extension("sqlite-shm"));
    }
}

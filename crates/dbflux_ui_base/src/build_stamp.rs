//! When, and from which commit, the running binary was built.
//!
//! The binary crate's build script records the values and installs them here
//! as a global at startup. They are not read with `env!` in this crate because
//! re-stamping a library would recompile every crate that depends on it.

use chrono::{DateTime, Local, TimeZone, Utc};
use gpui::Global;
use std::fmt::Display;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BuildStamp {
    built_at: DateTime<Utc>,
    git_sha: Option<String>,
}

impl Global for BuildStamp {}

impl BuildStamp {
    /// Parses the values the build script hands over: the build time in Unix
    /// seconds, and the abbreviated commit, empty when the tree had no git
    /// checkout. `None` when the time is not a valid timestamp.
    pub fn from_build_env(unix_time: &str, git_sha: &str) -> Option<Self> {
        let seconds = unix_time.trim().parse::<i64>().ok()?;
        let built_at = Utc.timestamp_opt(seconds, 0).single()?;
        let git_sha = Some(git_sha.trim())
            .filter(|sha| !sha.is_empty())
            .map(str::to_owned);

        Some(Self { built_at, git_sha })
    }

    /// `built 2026-10-30 11:21 · 8744286a`, in the viewer's local time zone so
    /// it reads the same as the clock next to it, wherever the build ran.
    ///
    /// Not translated: like the build profile shown in About, it identifies the
    /// binary rather than describing anything to the user.
    pub fn label(&self) -> String {
        self.label_in(&Local)
    }

    fn label_in<Tz>(&self, zone: &Tz) -> String
    where
        Tz: TimeZone,
        Tz::Offset: Display,
    {
        let built = self.built_at.with_timezone(zone).format("%Y-%m-%d %H:%M");

        match &self.git_sha {
            Some(sha) => format!("built {built} · {sha}"),
            None => format!("built {built}"),
        }
    }
}

/// The main window title: the app name, followed by the build stamp when the
/// binary carries one, so rebuilt binaries can be told apart at a glance.
pub fn main_window_title(app_name: &str, stamp: Option<&BuildStamp>) -> String {
    match stamp {
        Some(stamp) => format!("{app_name} · {}", stamp.label()),
        None => app_name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::FixedOffset;

    // 2026-10-30 11:21:05 UTC.
    const BUILT_AT: &str = "1793359265";

    #[test]
    fn test_label_shows_the_build_minute_and_the_commit() {
        let stamp = BuildStamp::from_build_env(BUILT_AT, "8744286a").unwrap();

        assert_eq!(stamp.label_in(&Utc), "built 2026-10-30 11:21 · 8744286a");
    }

    #[test]
    fn test_label_without_a_commit_shows_the_time_alone() {
        let stamp = BuildStamp::from_build_env(BUILT_AT, "").unwrap();

        assert_eq!(stamp.label_in(&Utc), "built 2026-10-30 11:21");
    }

    #[test]
    fn test_label_is_rendered_in_the_given_time_zone() {
        let stamp = BuildStamp::from_build_env(BUILT_AT, "8744286a").unwrap();
        let berlin_winter = FixedOffset::east_opt(3600).unwrap();

        assert_eq!(
            stamp.label_in(&berlin_winter),
            "built 2026-10-30 12:21 · 8744286a"
        );
    }

    #[test]
    fn test_from_build_env_trims_and_drops_a_blank_commit() {
        let stamp = BuildStamp::from_build_env(" 1793359265\n", "  \n").unwrap();

        assert_eq!(stamp.git_sha, None);
        assert_eq!(stamp.built_at.timestamp(), 1_793_359_265);
    }

    #[test]
    fn test_from_build_env_rejects_an_invalid_time() {
        assert_eq!(BuildStamp::from_build_env("", "8744286a"), None);
        assert_eq!(BuildStamp::from_build_env("yesterday", "8744286a"), None);
        assert_eq!(BuildStamp::from_build_env(&i64::MAX.to_string(), ""), None);
    }

    #[test]
    fn test_main_window_title_appends_the_stamp() {
        let stamp = BuildStamp::from_build_env(BUILT_AT, "8744286a").unwrap();

        assert_eq!(
            main_window_title("DBFlux", Some(&stamp)),
            format!("DBFlux · {}", stamp.label())
        );
        assert!(
            main_window_title("DBFlux Nightly", Some(&stamp))
                .starts_with("DBFlux Nightly · built ")
        );
    }

    #[test]
    fn test_main_window_title_without_a_stamp_is_the_app_name() {
        assert_eq!(main_window_title("DBFlux", None), "DBFlux");
    }
}

//! Packs on the engine (§23): the parts of turning a pack on or off that need the apply layer
//! (rule lines, placed files, settings) and the job scheduler. The state and checks are in
//! `crate::packs::ops`.
use super::*;
use crate::packs;

impl Engine {
    /// Read the packs of `files` (the group's repo files), place their folders and, on the
    /// host's machine, record what is in use. Never fails the sync: a pack problem is logged.
    pub(crate) fn pack_prepare(&self, id: &GroupId, files: &FileSet, host: Option<bool>) {
        let Some(home) = self.user_home.as_deref() else {
            return;
        };
        let prepare = packs::Prepare {
            home,
            host,
            now_ms: now_unix() * 1000,
        };
        match packs::prepare(&self.paths, id, files, &prepare) {
            Ok(r) if !r.carried.is_empty() => {
                tracing::info!(group = %id, packs = ?r.carried, "packs carried over");
            }
            Ok(_) => {}
            Err(e) => tracing::warn!(group = %id, error = %e, "could not read the group's packs"),
        }
    }

    /// Apply the group's last synced version again (after a pack turned on or off). No network.
    pub async fn reapply_group(&self, id: &GroupId) -> crate::Result<ApplyReport> {
        let _busy = self.busy.lock().await;
        self.reapply_locked(id)
    }

    fn reapply_locked(&self, id: &GroupId) -> crate::Result<ApplyReport> {
        let state = AppState::load(&self.paths)?;
        if state.disabled.contains(id) {
            return Ok(ApplyReport::default());
        }
        self.apply_base(id, &state.groups)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remote::fake::FakeGitHub;
    use serde_json::{json, Value};

    const PB: &str = "passbridge-signin@claude-mods";

    fn exit0() -> Vec<String> {
        if cfg!(windows) {
            vec!["cmd".into(), "/c".into(), "exit 0".into()]
        } else {
            vec!["sh".into(), "-c".into(), "exit 0".into()]
        }
    }

    /// A real apply layer on a temp `~/.claude`, joined to a group whose repo carries one pack.
    /// `host`: the engine's login owns the repo.
    async fn world(
        host: bool,
        local: Option<Value>,
    ) -> (tempfile::TempDir, Engine, GroupId, Paths) {
        let w = FakeGitHub::new();
        let (id, repo) = crate::engine::tests::group_repo(&w, &["ana", "bo"]);
        if host {
            repo.set_owner("bo");
        }
        let manifest = json!({
            "schema": 1, "id": "passbridge", "title": "PassBridge sign-in",
            "summary": "Signs in.", "plugins": [PB], "claudeMd": "CLAUDE.md",
            "setup": "setup.md", "check": {"run": exit0(), "timeoutSeconds": 30}
        });
        repo.set_file("packs/passbridge/pack.json", &manifest.to_string());
        repo.set_file("packs/passbridge/setup.md", "Set up.\n");
        repo.set_file("packs/passbridge/CLAUDE.md", "- then PassBridge\n");
        repo.set_file(
            group::SHARED_SETTINGS,
            &json!({"enabledPlugins": {PB: false}}).to_string(),
        );
        let dir = tempfile::tempdir().unwrap();
        let paths = Paths {
            claude_home: dir.path().join("claude"),
            state_dir: dir.path().join("state"),
        };
        if let Some(v) = local {
            std::fs::create_dir_all(&paths.claude_home).unwrap();
            std::fs::write(
                paths.claude_home.join("settings.json"),
                serde_json::to_vec(&v).unwrap(),
            )
            .unwrap();
        }
        let engine = Engine::with_remotes(
            paths.clone(),
            GitHub::with_api(String::new(), "http://127.0.0.1:9"),
            "bo".into(),
            "pc".into(),
            Arc::new(w.account("bo")),
        )
        .with_apply(Arc::new(FsApply))
        .with_scanner(|_| Vec::new())
        .with_user_home(dir.path().join("home"));
        engine.join(&id).await.unwrap();
        (dir, engine, id, paths)
    }

    fn plugin(paths: &Paths) -> Option<bool> {
        let b = std::fs::read(paths.claude_home.join("settings.json")).ok()?;
        crate::settings::parse_object(&b)?["enabledPlugins"][PB].as_bool()
    }

    #[tokio::test]
    async fn the_hosts_sync_keeps_what_runs_and_pushes_no_pack_file() {
        // The plugin is on here (the group had it on before the switch).
        let (_d, engine, id, paths) =
            world(true, Some(json!({"enabledPlugins": {PB: true}}))).await;
        let rec = crate::packs::state::record(&paths, &id, "passbridge")
            .unwrap()
            .unwrap();
        assert_eq!(rec.state, crate::packs::PackState::On);
        assert_eq!(rec.by, crate::packs::By::CarryOver);
        assert_eq!(plugin(&paths), Some(true));
        assert!(crate::watch::local_edits(&paths).unwrap().is_empty());
        // A second sync changes nothing and the repo gets no commit from the packs.
        let r = engine.sync_all().await.unwrap();
        assert!(
            r.groups.iter().all(|g| !g.pushed && g.error.is_none()),
            "{r:?}"
        );
        assert_eq!(plugin(&paths), Some(true));
    }

    #[tokio::test]
    async fn a_members_sync_turns_the_group_value_false_and_set_up_turns_it_on() {
        let (_d, engine, id, paths) =
            world(false, Some(json!({"enabledPlugins": {PB: true}}))).await;
        assert_eq!(
            plugin(&paths),
            Some(false),
            "the group's false applies to a member"
        );
        assert!(crate::packs::state::record(&paths, &id, "passbridge")
            .unwrap()
            .is_none());
        assert!(crate::packs::folder(&paths, &id, "passbridge")
            .join("setup.md")
            .is_file());

        let env = crate::packs::ops::Env::real(Path::new("/nowhere"));
        crate::packs::ops::start_setup(&paths, &id, "passbridge", &env).unwrap();
        assert!(
            crate::packs::ops::check_pack(&paths, &id, "passbridge", &env)
                .unwrap()
                .passed
        );
        let on = crate::packs::ops::turn_on(&paths, &id, "passbridge", &Default::default(), &env)
            .unwrap();
        assert!(on.plugins_changed);
        engine.reapply_group(&id).await.unwrap();
        assert_eq!(plugin(&paths), Some(true));
        let md = std::fs::read_to_string(paths.claude_home.join("CLAUDE.md")).unwrap();
        assert!(md.contains("packs/passbridge/CLAUDE.md"), "{md}");
        // A sync keeps it.
        engine.sync_all().await.unwrap();
        assert_eq!(plugin(&paths), Some(true));
        assert!(crate::watch::local_edits(&paths).unwrap().is_empty());

        assert!(crate::packs::ops::turn_off(&paths, &id, "passbridge", &env).unwrap());
        engine.reapply_group(&id).await.unwrap();
        assert_eq!(plugin(&paths), Some(false));
        let md = std::fs::read_to_string(paths.claude_home.join("CLAUDE.md")).unwrap();
        assert!(!md.contains("passbridge"), "{md}");
    }
}

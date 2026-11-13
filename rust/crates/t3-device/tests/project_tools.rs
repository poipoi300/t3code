#![allow(clippy::unwrap_used)]
mod common;
use async_trait::async_trait;
use common::*;
use serde_json::json;
use std::{path::Path, sync::Arc};
use t3_device::{
    io::CommandOutput,
    project::{find_eas_project_id, literal_ids},
    tools::{HUB_VERSION, ProcessInventory, Tool, Toolchain},
};
const A: &str = "d763fcb8-d37c-41ea-a773-b54a0ab4a454";
const B: &str = "54d70eaf-b0a3-477a-a9cd-2cf81a5c84c9";

#[test]
fn literal_project_scanner_rejects_computed_replaced_ambiguous_and_oversized_configs() {
    for(source,expected)in [
        (format!("export default {{extra:{{eas:{{projectId:'{A}'}}}}}};"),Some(vec![A.into()])),
        (format!("export default ({{ config }}) => ({{ ...config, extra: {{ ...config.extra, eas: {{ ...(config.extra?.eas ?? {{}}), projectId: '{A}' }} }} }});"),Some(vec![A.into()])),
        ("export default {extra:{eas:{projectId:process.env.ID}}}".into(),None),
        (format!("export default {{extra:{{eas:{{projectId:'{A}',...other}}}}}}"),None),
        (format!("export default {{extra:{{eas:{{projectId:'{A}'}}}}}};Object.assign(config.extra, other);"),None),
        (format!("config.extra={{eas:{{projectId:'{A}'}}}}"),None),
        (format!("export default {{extra:{{eas:{{projectId:'{A}' + suffix}}}}}}"),None),
        (format!("export default {{extra:{{eas:{{projectId:'{}' // prefix\u{2028}+ '{}' }}}}}}",&A[..9],&A[9..]),None),
        ("module.exports = ({config})=>({...config,...require('./local.js')});".into(),None),
        ("export default {name:'Mobile'}".into(),None),
        ("export default ({config})=>({...config,name:'Mobile'});".into(),Some(vec![])),
        ("module.exports=({config})=>({...config,extra:{...config?.['extra'],eas:{...config.extra?.eas,build:{experimental:true}},other:1}});".into(),Some(vec![])),
        (format!("export default {{extra:{{eas:{{projectId:`${{{A}}}`}}}}}}"),None),
        ("x".repeat(256*1024+1),None),
        ("projectId:".repeat(33),None),
    ]{assert_eq!(literal_ids(&source),expected,"{source:.300}");}
}
async fn write(root: &Path, path: &str, value: impl AsRef<[u8]>) {
    let path = root.join(path);
    tokio::fs::create_dir_all(path.parent().unwrap())
        .await
        .unwrap();
    tokio::fs::write(path, value).await.unwrap();
}
#[tokio::test]
async fn project_discovery_never_executes_dynamic_code_and_refuses_disagreeing_apps() {
    let temp = tempfile::tempdir().unwrap();
    write(temp.path(),"apps/mobile/app.config.ts",format!("throw new Error('config must not execute');export default {{extra:{{eas:{{projectId:'{A}'}}}}}};")).await;
    assert_eq!(find_eas_project_id(temp.path()).await.as_deref(), Some(A));
    write(
        temp.path(),
        "apps/kiosk/app.json",
        json!({"expo":{"extra":{"eas":{"projectId":B}}}}).to_string(),
    )
    .await;
    assert!(find_eas_project_id(temp.path()).await.is_none());
}
#[tokio::test]
async fn project_discovery_uses_first_static_config_and_unknown_apps_block_billing() {
    let temp = tempfile::tempdir().unwrap();
    write(
        temp.path(),
        "app.json",
        json!({"expo":{"extra":{"eas":{"projectId":A}}}}).to_string(),
    )
    .await;
    assert_eq!(find_eas_project_id(temp.path()).await.as_deref(), Some(A));
    write(
        temp.path(),
        "app.config.json",
        json!({"expo":{"name":"Mobile"}}).to_string(),
    )
    .await;
    assert!(find_eas_project_id(temp.path()).await.is_none());
    write(temp.path(), "apps/other/app.json", "// invalid JSON5").await;
    assert!(find_eas_project_id(temp.path()).await.is_none());
}
#[tokio::test]
async fn unreadable_static_config_cannot_be_ignored() {
    let temp = tempfile::tempdir().unwrap();
    tokio::fs::create_dir_all(temp.path().join("apps/one/app.json"))
        .await
        .unwrap();
    write(
        temp.path(),
        "apps/two/app.json",
        json!({"extra":{"eas":{"projectId":B}}}).to_string(),
    )
    .await;
    assert!(find_eas_project_id(temp.path()).await.is_none());
}

#[tokio::test]
async fn project_inheritance_refuses_wrong_holders_identifiers_and_malformed_static_objects(){
    for source in ["export default ({config})=>({...config,extra:{...config.extra.eas}})","export default ({config})=>({...config\u{301}})"]{
        let temp=tempfile::tempdir().unwrap();write(temp.path(),"app.config.js",source).await;
        write(temp.path(),"app.json",json!({"extra":{"eas":{"projectId":A}}}).to_string()).await;
        assert!(find_eas_project_id(temp.path()).await.is_none(),"{source}");
    }
    assert!(literal_ids(&format!("export default {{projectId:'{A}' /*unterminated")).is_none());
    for bad in [json!({"expo":null}),json!({"extra":[]}),json!({"extra":{"eas":[]}})]{
        let temp=tempfile::tempdir().unwrap();write(temp.path(),"app.json",json!({"extra":{"eas":{"projectId":A}}}).to_string()).await;
        write(temp.path(),"apps/unknown/app.json",bad.to_string()).await;
        assert!(find_eas_project_id(temp.path()).await.is_none(),"{bad}");
    }
}
#[tokio::test]
async fn failed_install_cleans_staging_and_never_exposes_registry_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let commands = FakeCommands::new(|c| {
        assert_eq!(c.timeout, std::time::Duration::from_secs(600));
        Ok(CommandOutput {
            code: 1,
            stderr: "registry https://private:credential@example.test/package".into(),
            ..Default::default()
        })
    });
    let tools = Toolchain::new(temp.path().into(), commands);
    let error = tools.ensure(Tool::Hub).await.err().unwrap();
    assert_eq!(
        error.to_string(),
        "Installing expo-device-hub failed while running npm install (exit code 1)."
    );
    assert!(!format!("{error:?}").contains("credential"));
    assert!(!tools.installed(Tool::Hub).await);
    assert!(
        std::fs::read_dir(temp.path().join("tools/expo-device-hub"))
            .unwrap()
            .next()
            .is_none()
    );
}
#[tokio::test]
async fn installation_requires_entry_and_sentinel_and_falls_back_only_for_missing_npm() {
    let temp = tempfile::tempdir().unwrap();
    let commands = FakeCommands::new(|c| {
        if c.program == "npm" {
            return Ok(CommandOutput {
                code: 127,
                ..Default::default()
            });
        }
        assert_eq!(c.program, "pnpm");
        assert_eq!(&c.args[..3], ["--package=npm@11", "dlx", "npm"]);
        let dir = std::path::PathBuf::from(
            &c.args[c.args.iter().position(|s| s == "--prefix").unwrap() + 1],
        );
        let entry = dir.join("node_modules/expo-device-hub/dist/server/cli.mjs");
        std::fs::create_dir_all(entry.parent().unwrap()).unwrap();
        std::fs::write(entry, "").unwrap();
        Ok(CommandOutput::default())
    });
    let tools = Toolchain::new(temp.path().into(), commands.clone());
    let installed = tools.ensure(Tool::Hub).await.unwrap();
    assert!(tools.installed(Tool::Hub).await);
    assert_eq!(
        std::fs::read_to_string(installed.sentinel).unwrap().trim(),
        HUB_VERSION
    );
    tools.ensure(Tool::Hub).await.unwrap();
    assert_eq!(commands.calls.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn inventory_includes_only_completed_versions_without_installing() {
    let temp = tempfile::tempdir().unwrap();
    for (version, sentinel) in [
        ("0.9.0", "0.9.0"),
        (HUB_VERSION, "wrong"),
        (".staging-123", ".staging-123"),
    ] {
        let paths = Tool::Hub.paths(temp.path(), version);
        tokio::fs::create_dir_all(paths.entry.parent().unwrap())
            .await
            .unwrap();
        tokio::fs::write(paths.entry, "").await.unwrap();
        tokio::fs::write(paths.sentinel, sentinel).await.unwrap();
    }
    let commands = FakeCommands::new(|_| panic!("inventory installed a tool"));
    let tools = Toolchain::new(temp.path().into(), commands);
    let inventory = tools.inventory(None, None).await.unwrap();
    assert_eq!(inventory.hub.installed_versions, ["0.9.0"]);
    assert!(inventory.agent.installed_versions.is_empty());
    assert!(!tools.installed(Tool::Hub).await);
}
struct Inventory {
    scan: Option<String>,
}
#[async_trait]
impl ProcessInventory for Inventory {
    async fn command_lines(&self) -> Option<String> {
        self.scan.clone()
    }
    async fn alive(&self, _: u32) -> bool {
        false
    }
}
#[tokio::test]
async fn maintenance_keeps_required_previous_incomplete_and_active_installs() {
    let temp = tempfile::tempdir().unwrap();
    for version in ["0.1.0", "0.2.0", "0.3.0", HUB_VERSION] {
        let paths = Tool::Hub.paths(temp.path(), version);
        tokio::fs::create_dir_all(&paths.install).await.unwrap();
        tokio::fs::write(paths.sentinel, version).await.unwrap();
    }
    let incomplete = Tool::Hub.paths(temp.path(), "0.4.0");
    tokio::fs::create_dir_all(&incomplete.install)
        .await
        .unwrap();
    let active = Tool::Hub.paths(temp.path(), "0.1.0");
    let tools = Arc::new(Toolchain::new(temp.path().into(), FakeCommands::encoder()));
    let inventory = Inventory {
        scan: Some(format!(
            "{}{}helper",
            active.install.display(),
            std::path::MAIN_SEPARATOR
        )),
    };
    tools.prune(Tool::Hub, &inventory).await.unwrap();
    assert!(active.install.exists());
    assert!(!Tool::Hub.paths(temp.path(), "0.2.0").install.exists());
    assert!(Tool::Hub.paths(temp.path(), "0.3.0").install.exists());
    assert!(incomplete.install.exists());
    assert!(Tool::Hub.paths(temp.path(), HUB_VERSION).install.exists());
    assert!(!temp.path().join("tools/.maintenance-lock").exists());
}
#[tokio::test]
async fn failed_process_inventory_and_absent_required_install_prevent_pruning() {
    for scan in [None, Some("node".into())] {
        let temp = tempfile::tempdir().unwrap();
        for version in ["0.1.0", "0.2.0"] {
            let paths = Tool::Hub.paths(temp.path(), version);
            tokio::fs::create_dir_all(&paths.install).await.unwrap();
            tokio::fs::write(paths.sentinel, version).await.unwrap();
        }
        let tools = Toolchain::new(temp.path().into(), FakeCommands::encoder());
        tools.prune(Tool::Hub, &Inventory { scan }).await.unwrap();
        assert!(Tool::Hub.paths(temp.path(), "0.1.0").install.exists());
        assert!(Tool::Hub.paths(temp.path(), "0.2.0").install.exists());
    }
}

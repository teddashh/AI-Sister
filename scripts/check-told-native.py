#!/usr/bin/env python3
"""Execute the desktop told command body against real core/config/SQLite on the host.
Only Tauri State and the shell DB holder are replaced; source is extracted afresh.
A154_RECALL_FIXTURE optionally exports actual retrieved hits for the renderer test.
"""
from pathlib import Path
import subprocess
import tempfile

root = Path(__file__).resolve().parent.parent
source = (root / 'apps/desktop/src-tauri/src/main.rs').read_text()

def item(start):
    begin = source.index(start)
    end = source.index('\n}', begin) + 2
    return source[begin:end]

command = item('fn remember_told(').replace("tauri::State<'_, Shell>", '&Shell')
outcome = item('enum RememberToldOutcome')
# Also compile and execute the real grounded citation mapper and its wire types.
mapper = item('fn synthesis_from_grounded(')
types = '\n'.join('#[derive(serde::Serialize)]\n' + item('struct ' + name + ' {')
                  for name in ['Hit', 'Fact', 'Moment', 'GroundedSynthesis', 'GroundedSentence', 'GroundedSource'])
reading_source = (root / 'apps/desktop/src-tauri/src/answer_readings.rs').read_text()
begin = reading_source.index('pub(crate) struct Reading {')
end = reading_source.index('\n}', begin) + 2
types += '\n#[derive(serde::Serialize)]\n' + reading_source[begin:end]
program = '''#![allow(dead_code, unused_imports)]
use std::{cell::RefCell, path::PathBuf};
use sister_core::{db::Db, config::Config};
thread_local! { static CONFIG: RefCell<PathBuf> = RefCell::new(PathBuf::new()); }
struct Shell { db: RefCell<Db> }
fn config_path() -> Result<PathBuf, String> { Ok(CONFIG.with(|p| p.borrow().clone())) }
fn with_db_mut<T>(shell: &&Shell, f: impl FnOnce(&mut Db) -> Result<T,String>) -> Result<T,String> {
    f(&mut shell.db.borrow_mut())
}
''' + '#[derive(serde::Serialize)]\n#[serde(rename_all="snake_case")]\n' + outcome + '\n' + command + '\n' + types + '\n' + mapper + '''
#[test]
fn a154_native_command_and_recall() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    CONFIG.with(|p| *p.borrow_mut() = path.clone());
    let mut config = Config::default();
    config.save(&path).unwrap();
    let shell = Shell { db: RefCell::new(Db::open_in_memory().unwrap()) };
    let owned = std::env::var("A154_TOLD_TEXT").unwrap_or_else(|_| "紫色雨傘在玄關 violetumbrella".into());
    let text = owned.as_str();
    let before = sister_core::now_ms();
    let result = remember_told(text.into(), &shell).unwrap();
    assert_eq!(serde_json::to_value(result).unwrap(), "remembered");
    let after = sister_core::now_ms();
    let hits = shell.db.borrow().search("紫色雨傘", 10).unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].text, text);
    assert!((before..=after).contains(&hits[0].ts));
    let openable = std::collections::HashSet::<i64>::new();
    let wire: Vec<Hit> = hits.into_iter()WIRE_MAP.collect();
    let moments = shell.db.borrow().timeline(before, after + 1, 10).unwrap();
    let moment_wire: Vec<Moment> = moments.into_iter()MOMENT_MAP.collect();
    assert_eq!(moment_wire.len(), 1);
    assert_eq!(moment_wire[0].source_kind, "told");
    assert_eq!(moment_wire[0].text, text);
    let synthesis = synthesis_from_grounded(sister_core::grounded_answer::GroundedAnswer {
        sentences: vec![sister_core::grounded_answer::GroundedSentence {
            text: text.into(), sources: vec![sister_core::grounded_answer::SourceRef::Chunk(wire[0].chunk_id)]
        }]
    }, &[], &[], &wire).unwrap();
    assert_eq!(synthesis.sentences[0].sources[0].label, "你告訴她的話");
    if let Ok(path) = std::env::var("A154_RECALL_FIXTURE") {
        std::fs::write(path, serde_json::to_vec(&serde_json::json!({"hits": wire, "synthesis": synthesis})).unwrap()).unwrap();
    }
    config.privacy.remember_told = false;
    config.save(&path).unwrap();
    assert_eq!(serde_json::to_value(remember_told("第二句".into(), &shell).unwrap()).unwrap(), "disabled");
    assert!(shell.db.borrow().search("第二句", 10).unwrap().is_empty());
    std::fs::write(&path, "invalid [ config").unwrap();
    assert!(remember_told("第三句".into(), &shell).is_err());
    assert!(shell.db.borrow().search("第三句", 10).unwrap().is_empty());
}
'''
wire_start = source.index('.map(|h| Hit {')
wire_end = source.index('})', wire_start) + 2
cli = (root / 'crates/sister-cli/src/ops.rs').read_text()
begin = cli.index('    fn origin_subject(')
end = cli.index('\n    }', begin) + len('\n    }')
program += cli[begin:end] + '\n' + r'''
#[test]
fn a154_cli_origin_matches_core() {
    let origin = sister_core::db::FactOrigin::from_target_row("told", None);
    assert_eq!(origin_subject(&origin), "你告訴她的話");
    assert_eq!(sister_core::db::target_provenance(Some(&sister_core::db::TargetApp::Known {
        app: "test".into(), origin
    })), "這個目標來自你告訴她的話");
}
'''
program = program.replace('WIRE_MAP', source[wire_start:wire_end])
moment_start = source.index('.map(|m| Moment {')
moment_end = source.index('})', moment_start) + 2
program = program.replace('MOMENT_MAP', source[moment_start:moment_end])
with tempfile.TemporaryDirectory(prefix='a154-native-') as temp:
    temp = Path(temp)
    (temp / 'src').mkdir()
    (temp / 'src/lib.rs').write_text(program)
    (temp / 'Cargo.toml').write_text(f'''[package]
name = "a154-native-probe"
version = "0.0.0"
edition = "2024"
[dependencies]
sister-core = {{ path = "{root / 'crates/sister-core'}" }}
serde = {{ version = "1", features = ["derive"] }}
serde_json = "1"
tempfile = "3"
''')
    result = subprocess.run(['cargo', 'test', '--lib', '--offline', '--manifest-path', str(temp / 'Cargo.toml'), '--target-dir', str(root / 'target'), '--', '--nocapture'], cwd=root)
    raise SystemExit(result.returncode)

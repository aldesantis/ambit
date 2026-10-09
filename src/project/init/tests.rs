use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use regex::Regex;
use serde_json::{Value, json};

use super::*;
use crate::errors::ExitCode;
use crate::model::catalog::{CatalogParseOptions, parse_catalog_directory};
use crate::model::config::CatalogRef;
use crate::model::config::parse_project_config;
use crate::model::pattern::PatternEntry;
use crate::model::requirement::ItemKind;
use crate::model::yaml::emit_yaml;
use crate::test_support::{CliResult, run_cli, tempdir, test_env};
use crate::util::fs::{EntryKind, lstat_kind, read_dir_names, read_text};

const SCAFFOLD_FILES: &[&str] = &[
    INIT_FILENAME,
    "hooks/.gitkeep",
    "mcps/.gitkeep",
    "packs/.gitkeep",
    "skills/.gitkeep",
];

const OTHER_CONFIG: &str = "ambit.yaml";

fn scaffold_values() -> Value {
    json!({
        "catalogs": [{ "name": "local", "source": "path:." }],
        "harnesses": ["claude"],
        "version": 1,
    })
}

fn with_example() -> Value {
    json!({
        "catalogs": [{ "name": "local", "source": "path:." }],
        "harnesses": ["claude"],
        "version": 1,
        "requires": [{ "pack": "local/*" }, { "skill": "local/*" }],
    })
}

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    project_dir: PathBuf,
}

fn fixture() -> Fixture {
    let dir = tempdir();
    let root = dir.path().to_path_buf();
    let project_dir = root.join("project");
    fs::create_dir_all(&project_dir).unwrap();

    Fixture {
        _dir: dir,
        root,
        project_dir,
    }
}

impl Fixture {
    fn cli(&self, argv: &[&str]) -> CliResult {
        let project = self.project_dir.to_string_lossy().into_owned();
        let mut args = argv.to_vec();
        args.extend(["--project", &project]);

        run_cli(&args, &self.root, &test_env(&self.root))
    }

    fn read(&self, file: &str) -> String {
        read_text(&self.project_dir.join(file)).unwrap()
    }

    fn read_config(&self) -> String {
        self.read(INIT_FILENAME)
    }
}

fn snapshot(dir: &Path) -> BTreeMap<String, String> {
    fn walk(inner: &Path, relative: &str, files: &mut BTreeMap<String, String>) {
        let mut names = read_dir_names(inner).unwrap();
        names.sort();

        for name in names {
            let next = if relative.is_empty() {
                name.clone()
            } else {
                format!("{relative}/{name}")
            };
            let path = inner.join(&name);

            if lstat_kind(&path).unwrap() == EntryKind::Dir {
                walk(&path, &next, files);
            } else {
                files.insert(next, read_text(&path).unwrap());
            }
        }
    }

    let mut files = BTreeMap::new();
    walk(dir, "", &mut files);
    files
}

fn listing(dir: &Path) -> Vec<String> {
    let mut names = read_dir_names(dir).unwrap();
    names.sort();
    names
}

fn values(text: &str) -> String {
    let lines: Vec<&str> = text
        .split('\n')
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect();

    format!("{}\n", lines.join("\n"))
}

fn uncommented(text: &str) -> String {
    let example = Regex::new(r"^# (?:requires:| )").unwrap();

    text.split('\n')
        .map(|line| {
            if example.is_match(line) {
                line[2..].to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn comment_above(text: &str, key: &str) -> Vec<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    let index = lines
        .iter()
        .position(|line| *line == format!("{key}:"))
        .unwrap_or_else(|| panic!("{key} is not a top-level key"));
    let mut comment = Vec::new();

    for line in lines[..index].iter().rev() {
        if !line.starts_with('#') {
            break;
        }

        comment.insert(0, (*line).to_owned());
    }

    comment
}

fn lines_out(lines: &[String]) -> String {
    lines.iter().map(|line| line.clone() + "\n").collect()
}

mod ambit_init {
    use super::*;

    #[test]
    fn writes_an_ambit_yml_the_config_loader_accepts() {
        let f = fixture();
        let result = f.cli(&["init"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        let config = parse_project_config(&f.read_config(), INIT_FILENAME).unwrap();

        assert_eq!(config.version, 1);
        assert_eq!(config.harnesses, ["claude"]);
        assert_eq!(
            config.catalogs,
            [CatalogRef {
                name: "local".to_owned(),
                source: "path:.".to_owned(),
                r#ref: None,
                path: None,
                trust: crate::model::config::Trust::Full,
            }]
        );
        assert_eq!(config.requires.len(), 0);
    }

    #[test]
    fn writes_the_config_and_every_item_directory_and_nothing_else() {
        let f = fixture();
        f.cli(&["init"]);

        let mut expected: Vec<&str> = SCAFFOLD_FILES.to_vec();
        expected.sort_unstable();
        let files: Vec<String> = snapshot(&f.project_dir).into_keys().collect();

        assert_eq!(files, expected);
    }

    #[test]
    fn scaffolds_a_catalog_the_parser_accepts_holding_nothing() {
        let f = fixture();
        f.cli(&["init"]);

        let catalog = parse_catalog_directory(
            "local",
            "path:.",
            &f.project_dir,
            None,
            &mut CatalogParseOptions::default(),
        )
        .unwrap();

        assert_eq!(catalog.skills.len(), 0);
        assert_eq!(catalog.mcps.len(), 0);
        assert_eq!(catalog.hooks.len(), 0);
    }

    #[test]
    fn scaffolds_a_project_ambit_validate_passes_against_with_no_edits() {
        let f = fixture();
        f.cli(&["init"]);

        let result = f.cli(&["validate"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert!(result.stdout.contains("problems (0)"));
    }

    #[test]
    fn is_read_by_the_commands_that_load_a_project_not_merely_by_the_parser() {
        let f = fixture();
        f.cli(&["init"]);

        let result = f.cli(&["search", "*"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert!(result.stdout.contains("local"));
    }

    #[test]
    fn holds_exactly_what_ambit_would_emit_from_those_values_plus_comments() {
        let f = fixture();
        f.cli(&["init"]);

        assert_eq!(values(&f.read_config()), emit_yaml(&scaffold_values()));
    }

    #[test]
    fn stays_sorted_and_parses_when_the_commented_out_example_is_uncommented() {
        let f = fixture();
        f.cli(&["init"]);
        let text = uncommented(&f.read_config());

        assert_eq!(values(&text), emit_yaml(&with_example()));

        let config = parse_project_config(&text, INIT_FILENAME).unwrap();

        assert_eq!(
            config.requires,
            [
                PatternEntry {
                    kind: ItemKind::Pack,
                    pattern: "*".to_owned(),
                    catalog: Some("local".to_owned()),
                },
                PatternEntry {
                    kind: ItemKind::Skill,
                    pattern: "*".to_owned(),
                    catalog: Some("local".to_owned()),
                },
            ]
        );
    }

    #[test]
    fn scaffolds_byte_identical_trees_into_two_fresh_directories() {
        let f = fixture();
        f.cli(&["init"]);
        let first = snapshot(&f.project_dir);

        let second = f.root.join("second");
        fs::create_dir_all(&second).unwrap();
        run_cli(
            &["init", "--project", &second.to_string_lossy()],
            &f.root,
            &test_env(&f.root),
        );

        assert_eq!(snapshot(&second), first);
        assert_eq!(first[INIT_FILENAME], scaffold_config());
    }

    #[test]
    fn explains_the_entry_grammar_above_the_commented_out_requires_block() {
        let f = fixture();
        f.cli(&["init"]);
        let text = f.read_config();
        let comment = comment_above(&text, "# requires").join("\n");

        assert!(text.contains("# requires:"));
        assert!(!Regex::new(r"(?m)^requires:").unwrap().is_match(&text));
        assert!(
            Regex::new(r"(?i)nothing is implicit")
                .unwrap()
                .is_match(&comment)
        );
        assert!(comment.contains("pack"));
        assert!(comment.contains("not `core` itself"));
    }

    #[test]
    fn prints_what_it_created_what_it_kept_and_the_two_things_left_to_do() {
        let f = fixture();
        let result = f.cli(&["init"]);
        let mut expected = vec![format!("created ({})", SCAFFOLD_FILES.len())];
        expected.extend(SCAFFOLD_FILES.iter().map(|file| format!("  {file}")));
        expected.extend(
            [
                "",
                "kept (0)",
                "  (none)",
                "",
                "next: put a skill in `skills/<name>/SKILL.md`, or add a catalog under `catalogs`",
                "      then uncomment a `requires` entry that selects it, and run `ambit install`",
            ]
            .map(str::to_owned),
        );

        assert_eq!(result.stdout, lines_out(&expected));
    }

    #[test]
    fn carries_every_files_bytes_in_json_so_a_consuming_tool_can_write_them_itself() {
        let f = fixture();
        let result = f.cli(&["init", "--json"]);
        let report: Value = serde_json::from_str(&result.stdout).unwrap();

        assert_eq!(report["written"], json!(true));
        assert_eq!(report["kept"], json!([]));

        let created = report["created"].as_array().unwrap();
        let files: Vec<&str> = created
            .iter()
            .map(|file| file["file"].as_str().unwrap())
            .collect();

        assert_eq!(files, SCAFFOLD_FILES);
        for file in created {
            assert_eq!(
                file["text"].as_str().unwrap(),
                f.read(file["file"].as_str().unwrap())
            );
        }
    }
}

mod on_a_directory_that_already_holds_a_config {
    use super::*;

    #[test]
    fn refuses_ambit_yml_leaving_it_byte_identical_and_writing_no_directories() {
        let f = fixture();
        fs::write(f.project_dir.join(INIT_FILENAME), "version: 1\n").unwrap();

        let result = f.cli(&["init"]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(
            result
                .stderr
                .contains(&format!("refusing to overwrite {INIT_FILENAME}"))
        );
        assert!(result.stderr.contains("ambit init"));
        assert_eq!(listing(&f.project_dir), [INIT_FILENAME]);
        assert_eq!(f.read_config(), "version: 1\n");
    }

    #[test]
    fn refuses_ambit_yaml_too_and_writes_no_ambit_yml_beside_it() {
        let f = fixture();
        fs::write(f.project_dir.join(OTHER_CONFIG), "version: 1\n").unwrap();

        let result = f.cli(&["init"]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(
            result
                .stderr
                .contains(&format!("refusing to overwrite {OTHER_CONFIG}"))
        );
        assert_eq!(listing(&f.project_dir), [OTHER_CONFIG]);
    }

    #[test]
    fn refuses_under_dry_run_as_well_since_the_preview_of_a_refusal_is_a_refusal() {
        let f = fixture();
        fs::write(f.project_dir.join(INIT_FILENAME), "version: 1\n").unwrap();

        let result = f.cli(&["init", "--dry-run"]);

        assert_eq!(result.code, ExitCode::Config);
        assert!(
            result
                .stderr
                .contains(&format!("refusing to overwrite {INIT_FILENAME}"))
        );
    }

    #[test]
    fn refuses_a_second_run_which_is_what_makes_the_config_the_refused_half() {
        let f = fixture();
        f.cli(&["init"]);
        let before = snapshot(&f.project_dir);

        let second = f.cli(&["init"]);

        assert_eq!(second.code, ExitCode::Config);
        assert_eq!(snapshot(&f.project_dir), before);
    }
}

mod on_a_directory_that_already_holds_a_gitkeep {
    use super::*;

    #[test]
    fn keeps_it_byte_identical_and_reports_it_rather_than_refusing() {
        let f = fixture();
        fs::create_dir_all(f.project_dir.join("skills")).unwrap();
        fs::write(f.project_dir.join("skills/.gitkeep"), "# mine\n").unwrap();

        let result = f.cli(&["init"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert!(result.stdout.contains("created (4)"));
        assert!(result.stdout.contains("kept (1)"));
        assert!(result.stdout.contains("  skills/.gitkeep"));
        assert_eq!(f.read("skills/.gitkeep"), "# mine\n");
    }

    #[test]
    fn leaves_an_occupied_item_directorys_other_contents_alone() {
        let f = fixture();
        fs::create_dir_all(f.project_dir.join("skills/mine")).unwrap();
        fs::write(f.project_dir.join("skills/mine/notes.md"), "# notes\n").unwrap();

        f.cli(&["init"]);

        assert_eq!(f.read("skills/mine/notes.md"), "# notes\n");
        assert_eq!(f.read("skills/.gitkeep"), "");
    }
}

mod dry_run {
    use super::*;

    #[test]
    fn prints_the_bytes_it_would_write_and_writes_none_of_them() {
        let f = fixture();
        let result = f.cli(&["init", "--dry-run"]);
        let mut expected = vec![format!("would create ({})", SCAFFOLD_FILES.len())];
        expected.extend(SCAFFOLD_FILES.iter().map(|file| format!("  {file}")));
        expected.extend(["", "kept (0)", "  (none)", ""].map(str::to_owned));
        expected.push(scaffold_config().trim_end().to_owned());

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert_eq!(result.stdout, lines_out(&expected));
        assert_eq!(listing(&f.project_dir).len(), 0);
    }

    #[test]
    fn reports_written_false_in_json_with_the_same_bytes_a_real_run_would_write() {
        let f = fixture();
        let preview = f.cli(&["init", "--dry-run", "--json"]);
        let previewed: Value = serde_json::from_str(&preview.stdout).unwrap();

        f.cli(&["init"]);

        assert_eq!(previewed["written"], json!(false));
        for file in previewed["created"].as_array().unwrap() {
            assert_eq!(
                file["text"].as_str().unwrap(),
                f.read(file["file"].as_str().unwrap())
            );
        }
    }
}

mod on_a_missing_directory {
    use super::*;

    #[test]
    fn refuses_it_rather_than_creating_one_and_names_it() {
        let f = fixture();
        let missing = f.root.join("absent");

        let result = run_cli(
            &["init", "--project", &missing.to_string_lossy()],
            &f.root,
            &test_env(&f.root),
        );

        assert_eq!(result.code, ExitCode::Config);
        assert!(
            result
                .stderr
                .contains(&format!("cannot initialize {}", missing.display()))
        );
        assert!(
            result
                .stderr
                .contains("`--project` at a directory that exists")
        );
        assert!(read_dir_names(&missing).is_err());
    }
}

mod the_scaffold_as_a_value {
    use super::*;

    #[test]
    fn is_a_function_of_nothing_in_path_order() {
        let files: Vec<String> = scaffold_project()
            .into_iter()
            .map(|file| file.file)
            .collect();

        assert_eq!(files, SCAFFOLD_FILES);
        assert_eq!(scaffold_project(), scaffold_project());
    }
}

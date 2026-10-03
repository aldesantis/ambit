//! `ambit init`: the scaffolded project, which is also a catalog.
//!
//! Four claims carry this suite, and none of them is about the prose.
//!
//! The first is that the scaffold is *emitted*: strip its comments and what is left must be
//! byte-identical to what `emit_yaml` produces from the same values, so the file cannot drift into
//! an unsorted key or an unquoted value and cannot stop being byte-stable between runs. That holds
//! for the commented-out block too, which must be valid config the moment the `# ` comes off.
//!
//! The second is that the two halves agree. The scaffolded `catalogs:` entry is live and names the
//! project itself, so the item directories have to be there for it to be true; the `requires`
//! entry selecting that catalog is commented, because an entry matching nothing is exit 3 and a
//! fresh project's own catalog is empty. Both are checked by running `ambit validate` against the
//! result.
//!
//! The third is that it still teaches the entry grammar, which is the part that costs a bundle when
//! it goes missing.
//!
//! The fourth is about what it leaves alone: an existing config is refused, an existing `.gitkeep`
//! is kept byte-identical and reported, and a missing project root is refused rather than created.
//!
//! The prose itself is deliberately not pinned. It is documentation, free to be reworded; what is
//! pinned is that a comment adjacent to `requires` says nothing is implicit and explains the glob
//! rule.

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

/// Every file the scaffold writes, in the order the command reports them.
const SCAFFOLD_FILES: &[&str] = &[
    INIT_FILENAME,
    "hooks/.gitkeep",
    "mcps/.gitkeep",
    "packs/.gitkeep",
    "skills/.gitkeep",
];

const OTHER_CONFIG: &str = "ambit.yaml";

/// What the scaffold sets, stated here rather than imported so the test is an independent claim.
///
/// `catalogs` is live: every project is a catalog, and the entry is what makes its own `packs/`,
/// `skills/`, `mcps/` and `hooks/` reachable.
fn scaffold_values() -> Value {
    json!({
        "catalogs": [{ "name": "local", "source": "path:." }],
        "harnesses": ["claude"],
        "version": 1,
    })
}

/// The scaffold with its commented-out example uncommented, which is what a reader does.
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
    /// Runs the CLI against the project, collecting stdout and stderr.
    fn cli(&self, argv: &[&str]) -> CliResult {
        let project = self.project_dir.to_string_lossy().into_owned();
        let mut args = argv.to_vec();
        args.extend(["--project", &project]);

        run_cli(&args, &self.root, &test_env(&self.root))
    }

    /// One project-relative file's bytes.
    fn read(&self, file: &str) -> String {
        read_text(&self.project_dir.join(file)).unwrap()
    }

    fn read_config(&self) -> String {
        self.read(INIT_FILENAME)
    }
}

/// Every file under `dir` with its bytes, so a whole tree can be compared or asserted unchanged.
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

/// The document with every comment and separator dropped: the values, as YAML.
fn values(text: &str) -> String {
    let lines: Vec<&str> = text
        .split('\n')
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .collect();

    format!("{}\n", lines.join("\n"))
}

/// The scaffold with its commented-out example turned back into config.
///
/// A comment line belongs to the example rather than to the prose when what follows `# ` is either
/// the key itself or further indentation: prose never begins with a space. `requires` is the only
/// key shown that way: `catalogs` is scaffolded live.
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

/// The contiguous comment lines immediately above a key.
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
    #[ignore = "needs B1"]
    fn writes_an_ambit_yml_the_config_loader_accepts() {
        let f = fixture();
        let result = f.cli(&["init"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);

        let config = parse_project_config(&f.read_config(), INIT_FILENAME).unwrap();

        assert_eq!(config.version, 1);
        assert_eq!(config.harnesses, ["claude"]);
        // The project lists itself, which is the only way a project ships a skill of its own.
        assert_eq!(
            config.catalogs,
            [CatalogRef {
                name: "local".to_owned(),
                source: "path:.".to_owned(),
                r#ref: None,
            }]
        );
        // Nothing selected, which is what keeps `ambit validate` clean on a fresh project: an entry
        // matching nothing is exit 3, and `local` is empty directories.
        assert_eq!(config.requires.len(), 0);
    }

    #[test]
    #[ignore = "needs B1"]
    fn writes_the_config_and_every_item_directory_and_nothing_else() {
        let f = fixture();
        f.cli(&["init"]);

        let mut expected: Vec<&str> = SCAFFOLD_FILES.to_vec();
        expected.sort_unstable();
        let files: Vec<String> = snapshot(&f.project_dir).into_keys().collect();

        assert_eq!(files, expected);
    }

    #[test]
    #[ignore = "needs B1"]
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

        // A `.gitkeep` is invisible to parsing, so the directories are additive rather than a
        // catalog declaring something nobody wrote.
        assert_eq!(catalog.skills.len(), 0);
        assert_eq!(catalog.mcps.len(), 0);
        assert_eq!(catalog.hooks.len(), 0);
    }

    #[test]
    #[ignore = "needs B1, B2"]
    fn scaffolds_a_project_ambit_validate_passes_against_with_no_edits() {
        let f = fixture();
        f.cli(&["init"]);

        let result = f.cli(&["validate"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert!(result.stdout.contains("problems (0)"));
    }

    #[test]
    #[ignore = "needs B1, B2"]
    fn is_read_by_the_commands_that_load_a_project_not_merely_by_the_parser() {
        let f = fixture();
        f.cli(&["init"]);

        // `ambit search` loads the config the way every command does, so a scaffold it accepts is
        // one the whole tool accepts, and what it dumps is the project's own empty catalog.
        let result = f.cli(&["search", "*"]);

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        assert!(result.stdout.contains("local"));
    }

    #[test]
    #[ignore = "needs B1"]
    fn holds_exactly_what_ambit_would_emit_from_those_values_plus_comments() {
        let f = fixture();
        f.cli(&["init"]);

        assert_eq!(values(&f.read_config()), emit_yaml(&scaffold_values()));
    }

    #[test]
    #[ignore = "needs B1"]
    fn stays_sorted_and_parses_when_the_commented_out_example_is_uncommented() {
        let f = fixture();
        f.cli(&["init"]);
        let text = uncommented(&f.read_config());

        assert_eq!(values(&text), emit_yaml(&with_example()));

        let config = parse_project_config(&text, INIT_FILENAME).unwrap();

        // The `requires` example quotes the alias the live `catalogs` block declares, so
        // uncommenting it leaves a config that agrees with itself.
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
    #[ignore = "needs B1"]
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
    #[ignore = "needs B1"]
    fn explains_the_entry_grammar_above_the_commented_out_requires_block() {
        let f = fixture();
        f.cli(&["init"]);
        let text = f.read_config();
        let comment = comment_above(&text, "# requires").join("\n");

        // Commented, so the scaffold selects nothing; and the prose is where the two declarations
        // and the glob rule are stated, since nothing warns about either at install time.
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
    #[ignore = "needs B1"]
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
    #[ignore = "needs B1"]
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
    #[ignore = "needs B1"]
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
        // The config is the file that makes a directory a project, so a refusal is total: not the
        // config, and not a `.gitkeep` beside it.
        assert_eq!(listing(&f.project_dir), [INIT_FILENAME]);
        assert_eq!(f.read_config(), "version: 1\n");
    }

    #[test]
    #[ignore = "needs B1"]
    fn refuses_ambit_yaml_too_and_writes_no_ambit_yml_beside_it() {
        // Both names are accepted config, so scaffolding the other one would leave a project whose
        // two configs are an error in every other command.
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
    #[ignore = "needs B1"]
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
    #[ignore = "needs B1"]
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
    #[ignore = "needs B1"]
    fn keeps_it_byte_identical_and_reports_it_rather_than_refusing() {
        // A `.gitkeep` carries no bytes to lose and is exactly what a project with its own
        // `skills/` already has, so it is kept where a config would be refused.
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
    #[ignore = "needs B1"]
    fn leaves_an_occupied_item_directorys_other_contents_alone() {
        let f = fixture();
        fs::create_dir_all(f.project_dir.join("skills/mine")).unwrap();
        fs::write(f.project_dir.join("skills/mine/notes.md"), "# notes\n").unwrap();

        f.cli(&["init"]);

        assert_eq!(f.read("skills/mine/notes.md"), "# notes\n");
        // The directory was there, but the `.gitkeep` inside it was not, so it is created.
        assert_eq!(f.read("skills/.gitkeep"), "");
    }
}

mod dry_run {
    use super::*;

    #[test]
    #[ignore = "needs B1"]
    fn prints_the_bytes_it_would_write_and_writes_none_of_them() {
        let f = fixture();
        let result = f.cli(&["init", "--dry-run"]);
        let mut expected = vec![format!("would create ({})", SCAFFOLD_FILES.len())];
        expected.extend(SCAFFOLD_FILES.iter().map(|file| format!("  {file}")));
        expected.extend(["", "kept (0)", "  (none)", ""].map(str::to_owned));
        expected.push(scaffold_config().trim_end().to_owned());

        assert_eq!(result.code, ExitCode::Success, "{}", result.stderr);
        // Each printed line is one stdout line; the config's own newlines come out unchanged.
        assert_eq!(result.stdout, lines_out(&expected));
        // Not the config, and not one of the directories the `.gitkeep` files would create.
        assert_eq!(listing(&f.project_dir).len(), 0);
    }

    #[test]
    #[ignore = "needs B1"]
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
    #[ignore = "needs B1"]
    fn refuses_it_rather_than_creating_one_and_names_it() {
        // `--project` naming the wrong path should not leave a project (directories and a config)
        // where nobody meant.
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
    #[ignore = "needs B1"]
    fn is_a_function_of_nothing_in_path_order() {
        // Two runs into two differently named directories must produce identical trees, so nothing
        // about the target (a directory name, an absolute path, a timestamp) may reach the bytes.
        let files: Vec<String> = scaffold_project()
            .into_iter()
            .map(|file| file.file)
            .collect();

        assert_eq!(files, SCAFFOLD_FILES);
        assert_eq!(scaffold_project(), scaffold_project());
    }
}

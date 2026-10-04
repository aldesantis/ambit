//! The managed `.gitignore` block, as a text transformation.
//!
//! The whole claim is about lines ambit does not own: they must come back byte for byte, wherever
//! they sit relative to the block. So every case here pins the *surrounding* file as well as the
//! block, and the two ambiguous shapes (a second block, a block whose end marker is gone) are
//! asserted to stop rather than to pick a span of lines.
//!
//! The install-time behaviour is in `project/install/tests.rs`; this is the part that needs no
//! filesystem.

use pretty_assertions::assert_eq;

use super::*;
use crate::errors::{AmbitError, ExitCode};
use crate::model::state::ArtifactMode;

const SKILLS_DIR: &str = ".agents/skills";

fn entries() -> Vec<String> {
    vec![
        ".ambit/".to_owned(),
        format!("{SKILLS_DIR}/company-context"),
    ]
}

fn strings(list: &[&str]) -> Vec<String> {
    list.iter().map(|&item| item.to_owned()).collect()
}

fn update(existing: Option<&str>, entries: &[String]) -> Option<String> {
    update_gitignore_text(existing, entries, GITIGNORE_FILENAME).expect("an unambiguous file")
}

fn remove(existing: Option<&str>) -> Option<String> {
    remove_gitignore_text(existing, GITIGNORE_FILENAME).expect("an unambiguous file")
}

/// The file as lines, which is how every assertion here reads. The begin marker is replaced by
/// [`BLOCK_BEGIN`] after checking it starts with it, standing in for `expect.stringContaining`.
fn lines(text: Option<String>) -> Vec<String> {
    let text = text.expect("a rewritten file");

    text.split('\n')
        .map(|line| {
            if line.contains(BLOCK_BEGIN) {
                BLOCK_BEGIN.to_owned()
            } else {
                line.to_owned()
            }
        })
        .collect()
}

/// Asserts the result rejected the file as a config error (exit 2).
fn rejection<T: std::fmt::Debug>(result: Result<T>) -> AmbitError {
    let error = result.expect_err("expected a rejection");

    assert_eq!(
        error.code,
        ExitCode::Config,
        "expected exit 2: {}",
        error.format()
    );
    error
}

fn dir(path: &str, kind: ArtifactKind, mode: ArtifactMode) -> OwnedArtifact {
    OwnedArtifact {
        path: path.to_owned(),
        kind,
        mode: Some(mode),
        managed_keys: None,
        format: None,
        shape: None,
        digest: None,
    }
}

fn block(file: &str, entries: &[&str]) -> IgnoreBlock {
    IgnoreBlock {
        file: file.to_owned(),
        entries: strings(entries),
    }
}

// the managed blocks' contents

#[test]
fn lists_every_installed_skill_directory_in_the_shared_directorys_own_file() {
    let artifacts = [
        dir(
            &format!("{SKILLS_DIR}/company-context"),
            ArtifactKind::SkillDir,
            ArtifactMode::Link,
        ),
        dir(
            &format!("{SKILLS_DIR}/design-tokens"),
            ArtifactKind::SkillDir,
            ArtifactMode::Copy,
        ),
    ];

    assert_eq!(
        gitignore_blocks(&artifacts),
        [
            block(GITIGNORE_FILENAME, &[".ambit/"]),
            block(
                SHARED_GITIGNORE_FILE,
                &["/skills/company-context", "/skills/design-tokens"]
            ),
        ]
    );
}

#[test]
fn anchors_a_shared_entry_with_a_leading_slash_so_it_cannot_match_at_another_depth() {
    let artifacts = [dir(
        &format!("{SKILLS_DIR}/skills"),
        ArtifactKind::SkillDir,
        ArtifactMode::Copy,
    )];

    assert_eq!(gitignore_blocks(&artifacts)[1].entries, ["/skills/skills"]);
}

#[test]
fn keeps_at_the_root_what_a_nested_file_cannot_reach_the_state_dir_and_the_skills_link() {
    let artifacts = [
        dir(
            &format!("{SKILLS_DIR}/company-context"),
            ArtifactKind::SkillDir,
            ArtifactMode::Link,
        ),
        dir(
            ".claude/skills",
            ArtifactKind::SkillsLink,
            ArtifactMode::Link,
        ),
    ];

    assert_eq!(
        gitignore_blocks(&artifacts)[0],
        block(GITIGNORE_FILENAME, &[".ambit/", ".claude/skills"])
    );
}

#[test]
fn leaves_a_co_owned_config_file_out_since_mcp_json_is_a_file_a_team_commits() {
    let artifacts = [OwnedArtifact {
        path: ".mcp.json".to_owned(),
        kind: ArtifactKind::HarnessConfig,
        mode: None,
        managed_keys: Some(strings(&["mcpServers.tagged"])),
        format: None,
        shape: None,
        digest: None,
    }];

    assert_eq!(
        gitignore_blocks(&artifacts),
        [
            block(GITIGNORE_FILENAME, &[".ambit/"]),
            block(SHARED_GITIGNORE_FILE, &[]),
        ]
    );
}

#[test]
fn never_lists_the_nested_file_itself_since_it_is_generated_but_tracked() {
    let artifacts = [dir(
        &format!("{SKILLS_DIR}/company-context"),
        ArtifactKind::SkillDir,
        ArtifactMode::Link,
    )];

    for block in gitignore_blocks(&artifacts) {
        assert!(!block.entries.contains(&SHARED_GITIGNORE_FILE.to_owned()));
        assert!(!block.entries.contains(&format!("/{GITIGNORE_FILENAME}")));
    }
}

#[test]
fn gives_a_skill_directory_no_trailing_slash_so_the_pattern_also_covers_a_symlinked_one() {
    let artifacts = [dir(
        &format!("{SKILLS_DIR}/company-context"),
        ArtifactKind::SkillDir,
        ArtifactMode::Link,
    )];

    assert!(
        !gitignore_blocks(&artifacts)[1]
            .entries
            .contains(&"/skills/company-context/".to_owned())
    );
}

// an empty block

#[test]
fn renders_no_block_at_all_rather_than_a_pair_of_markers_with_nothing_between_them() {
    assert_eq!(
        update_gitignore_text(None, &[], SHARED_GITIGNORE_FILE).unwrap(),
        None
    );
}

#[test]
fn takes_an_existing_block_back_out_so_a_project_that_deselected_every_skill_loses_the_file() {
    let existing = format!("{BLOCK_BEGIN}\n/skills/company-context\n{BLOCK_END}\n");

    assert_eq!(
        update_gitignore_text(Some(&existing), &[], SHARED_GITIGNORE_FILE).unwrap(),
        Some(String::new())
    );
}

#[test]
fn leaves_lines_it_does_not_own_behind_when_it_removes_the_block() {
    let existing = format!("# theirs\n\n{BLOCK_BEGIN}\n/skills/company-context\n{BLOCK_END}\n");

    assert_eq!(
        update_gitignore_text(Some(&existing), &[], SHARED_GITIGNORE_FILE).unwrap(),
        Some("# theirs\n".to_owned())
    );
}

// writing the block into a file that has none

#[test]
fn creates_the_whole_file_when_the_project_has_no_gitignore() {
    assert_eq!(
        lines(update(None, &entries())),
        [
            BLOCK_BEGIN,
            &format!("{SKILLS_DIR}/company-context"),
            ".ambit/",
            BLOCK_END,
            ""
        ]
    );
}

#[test]
fn appends_after_the_lines_already_there_separated_by_one_blank_line() {
    assert_eq!(
        lines(update(Some("node_modules/\ndist/\n"), &entries())),
        [
            "node_modules/",
            "dist/",
            "",
            BLOCK_BEGIN,
            &format!("{SKILLS_DIR}/company-context"),
            ".ambit/",
            BLOCK_END,
            ""
        ]
    );
}

#[test]
fn adds_no_second_blank_line_to_a_file_that_already_ends_in_one() {
    assert_eq!(
        lines(update(Some("node_modules/\n\n"), &entries())),
        [
            "node_modules/",
            "",
            BLOCK_BEGIN,
            &format!("{SKILLS_DIR}/company-context"),
            ".ambit/",
            BLOCK_END,
            ""
        ]
    );
}

#[test]
fn gives_a_file_with_no_trailing_newline_one_rather_than_joining_onto_its_last_line() {
    assert_eq!(
        lines(update(Some("dist/"), &entries())),
        [
            "dist/",
            "",
            BLOCK_BEGIN,
            &format!("{SKILLS_DIR}/company-context"),
            ".ambit/",
            BLOCK_END,
            ""
        ]
    );
}

#[test]
fn sorts_and_deduplicates_the_paths_so_two_identical_installs_write_identical_bytes() {
    let shuffled = vec![
        format!("{SKILLS_DIR}/b"),
        ".ambit/".to_owned(),
        format!("{SKILLS_DIR}/a"),
        format!("{SKILLS_DIR}/b"),
    ];
    let sorted = vec![
        ".ambit/".to_owned(),
        format!("{SKILLS_DIR}/a"),
        format!("{SKILLS_DIR}/b"),
    ];

    assert_eq!(update(None, &shuffled), update(None, &sorted));
}

#[test]
fn escapes_the_glob_characters_that_would_make_a_path_match_something_else() {
    assert!(
        lines(update(None, &[format!("{SKILLS_DIR}/weird[name]")]))
            .contains(&format!("{SKILLS_DIR}/weird\\[name]"))
    );
}

// rewriting a block that is already there

fn installed() -> String {
    update(Some("node_modules/\n"), &entries()).unwrap_or_default()
}

#[test]
fn replaces_the_block_in_place_disturbing_nothing_above_or_below_it() {
    let surrounded = format!("{}\n# mine\ncoverage/\n", installed());

    assert_eq!(
        lines(update(
            Some(&surrounded),
            &[".ambit/".to_owned(), format!("{SKILLS_DIR}/code-review")]
        )),
        [
            "node_modules/",
            "",
            BLOCK_BEGIN,
            &format!("{SKILLS_DIR}/code-review"),
            ".ambit/",
            BLOCK_END,
            "",
            "# mine",
            "coverage/",
            ""
        ]
    );
}

#[test]
fn drops_a_path_the_new_install_no_longer_owns() {
    let narrowed = update(Some(&installed()), &strings(&[".ambit/"])).unwrap_or_default();

    assert!(!narrowed.contains("company-context"));
    assert_eq!(
        lines(Some(narrowed)),
        ["node_modules/", "", BLOCK_BEGIN, ".ambit/", BLOCK_END, ""]
    );
}

#[test]
fn reports_no_change_when_the_block_already_says_what_this_install_would_write() {
    assert_eq!(update(Some(&installed()), &entries()), None);
}

#[test]
fn rewrites_a_block_whose_opening_line_was_written_by_another_version_of_ambit() {
    let older = format!("{BLOCK_BEGIN}\n.ambit/\n{BLOCK_END}\n");

    assert_eq!(
        lines(update(Some(&older), &entries())),
        [
            BLOCK_BEGIN,
            &format!("{SKILLS_DIR}/company-context"),
            ".ambit/",
            BLOCK_END,
            ""
        ]
    );
}

#[test]
fn keeps_a_crlf_files_line_endings_on_the_lines_it_does_not_own() {
    let crlf = format!("node_modules/\r\ndist/\r\n{BLOCK_BEGIN}\r\n.ambit/\r\n{BLOCK_END}\r\n");

    assert_eq!(
        lines(update(Some(&crlf), &entries())),
        [
            "node_modules/\r",
            "dist/\r",
            BLOCK_BEGIN,
            &format!("{SKILLS_DIR}/company-context"),
            ".ambit/",
            BLOCK_END,
            ""
        ]
    );
}

// removing the block
//
// The claim is the inverse of writing it: install then clean must give a file back exactly as it
// was, so every case here compares against the *input* to `update_gitignore_text` rather than
// against a hand-written expectation of what removal should leave.

#[test]
fn gives_a_file_that_had_lines_of_its_own_back_byte_for_byte() {
    let before = "node_modules/\ndist/\n";

    assert_eq!(
        remove(update(Some(before), &entries()).as_deref()),
        Some(before.to_owned())
    );
}

#[test]
fn reports_an_empty_file_when_the_block_was_the_whole_of_it_so_the_caller_deletes_it() {
    assert_eq!(
        remove(update(None, &entries()).as_deref()),
        Some(String::new())
    );
}

#[test]
fn leaves_the_lines_below_the_block_where_they_were() {
    let surrounded = format!(
        "{}\n# mine\ncoverage/\n",
        update(Some("node_modules/\n"), &entries()).unwrap_or_default()
    );

    assert_eq!(
        lines(remove(Some(&surrounded))),
        ["node_modules/", "", "# mine", "coverage/", ""]
    );
}

#[test]
fn reports_no_change_for_a_file_that_holds_no_block() {
    assert_eq!(remove(Some("node_modules/\n")), None);
    assert_eq!(remove(None), None);
}

#[test]
fn exits_2_rather_than_guessing_at_an_unterminated_block() {
    let orphaned = format!("dist/\n{BLOCK_BEGIN}\n.ambit/\ncoverage/\n");

    assert!(
        rejection(remove_gitignore_text(Some(&orphaned), GITIGNORE_FILENAME))
            .message
            .contains("unterminated ambit block")
    );
}

// a .gitignore whose markers cannot be read

#[test]
fn exits_2_rather_than_choosing_between_two_blocks() {
    let two = format!(
        "{BLOCK_BEGIN}\n.ambit/\n{BLOCK_END}\ndist/\n{BLOCK_BEGIN}\n.ambit/\n{BLOCK_END}\n"
    );

    let error = rejection(update_gitignore_text(
        Some(&two),
        &entries(),
        GITIGNORE_FILENAME,
    ));

    assert!(error.message.contains("more than one ambit block"));
    assert!(error.format().contains("line 1 and line 5"));
}

#[test]
fn exits_2_rather_than_swallowing_every_line_below_an_unterminated_block() {
    let orphaned = format!("dist/\n{BLOCK_BEGIN}\n.ambit/\ncoverage/\n");

    let error = rejection(update_gitignore_text(
        Some(&orphaned),
        &entries(),
        GITIGNORE_FILENAME,
    ));

    assert!(error.message.contains("unterminated ambit block"));
    assert!(error.format().contains("line 2"));
    assert!(error.format().contains(BLOCK_END));
}

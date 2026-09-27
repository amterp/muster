//! Whether the documentation this binary ships is all of the documentation there is.
//!
//! `include_str!` fails loudly on a file that is gone, and silently on a file nobody names: the
//! markdown sits in the repo, gets reviewed as prose, and is never embedded. So this walks
//! `docs/cli/` and fails on anything the binary cannot print - the same rule `corpus-lint`
//! applies to a corpus file no driver loads, for the same reason.

use std::collections::BTreeSet;

use muster_cli::docs;

#[test]
fn every_document_in_the_repo_is_one_this_binary_can_print() {
    let directory = conformance::repo_root().join("docs/cli");
    let on_disk: BTreeSet<String> = std::fs::read_dir(&directory)
        .unwrap_or_else(|error| panic!("{} could not be read: {error}", directory.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|kind| kind.eq_ignore_ascii_case("md")))
        .filter_map(|path| path.file_stem().map(|name| name.to_string_lossy().into_owned()))
        .collect();
    let embedded: BTreeSet<String> =
        docs::TOPICS.iter().map(|topic| topic.name.to_string()).collect();

    assert_eq!(
        on_disk, embedded,
        "docs/cli/ and the topics in docs.rs disagree. A file nobody names is documentation \
         somebody wrote, reviewed and shipped without, and `muster docs` is the only place \
         anybody would look for it."
    );
    assert!(!embedded.is_empty(), "an empty topic list would pass every check above");
}

#[test]
fn every_document_says_something_and_says_what_it_is() {
    for topic in docs::TOPICS {
        assert!(
            topic.text.len() > 200,
            "the `{}` document is {} bytes, which is a stub rather than an answer",
            topic.name,
            topic.text.len()
        );
        assert!(
            topic.text.starts_with("# "),
            "the `{}` document opens with {:?} rather than a heading. `muster docs all` runs \
             them together, so a document with no title is one nobody can tell from the one \
             before it.",
            topic.name,
            topic.text.lines().next().unwrap_or_default()
        );
        assert!(
            !topic.about.is_empty(),
            "the `{}` topic is listed under nothing, so `muster docs` gives no reason to open it",
            topic.name
        );
    }
}

#[test]
fn the_listing_names_every_document_and_the_refusal_does_too() {
    let listing = docs::listing();
    let refusal = docs::no_such_topic("oveview");
    for topic in docs::TOPICS {
        assert!(listing.contains(topic.name), "`muster docs` does not list `{}`", topic.name);
        assert!(
            refusal.contains(topic.name),
            "a mistyped topic is refused without mentioning `{}`, so whoever typed it has to \
             guess again",
            topic.name
        );
    }
}

#[test]
fn all_of_them_is_all_of_them() {
    let everything = docs::everything();
    for topic in docs::TOPICS {
        assert!(
            everything.contains(topic.text.trim_end()),
            "`muster docs all` left out `{}`, which makes it the one command here that quietly \
             answers less than it says",
            topic.name
        );
    }
}

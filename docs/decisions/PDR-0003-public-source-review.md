---
id: "PDR-0003"
title: "Public source and test-data review"
status: "accepted"
date: "2026-09-25"
deciders:
  - "@3leapsdave"
scope: "repository source, tests, documentation, and publication metadata"
tags:
  - "process"
  - "source"
  - "testing"
relates-to:
  - "https://github.com/3leaps/oss-policies/blob/main/SENSITIVE-LOCAL-DATA.md"
---

# PDR-0003: Public source and test-data review

## Decision

This repository follows the [3 Leaps OSS Sensitive Local Data Policy](https://github.com/3leaps/oss-policies/blob/main/SENSITIVE-LOCAL-DATA.md).
Tests, fixtures, snapshots, examples, comments, documentation, and diagnostics
must not contain private filesystem paths, home-directory names, internal hosts
or URLs, non-public repository references, planning identifiers, or identifying
customer or operational data. The same rule applies to expected strings,
regular expressions, deny lists, and negative tests. Encoding, splitting,
hashing, or otherwise disguising an actual private reference does not make it
acceptable test data.

Use independently invented synthetic values, reserved example domains, and
temporary paths created by the test harness. Required external integration
inputs are supplied by documented configuration; their values and captured
contents do not enter committed fixtures or public diagnostics. Proprietary
material stays outside the repository working tree, even when an ignore rule
would hide it. Legitimate public API names, standard filesystem locations, and
repository-relative paths remain usable when they disclose no private context.

## Review and checks

The public-source check scans tracked text across source, comments, scripts,
tests, fixtures, and documentation, including decision records and the check
itself. There are no path exclusions. Binary files, filenames, ignored and
untracked files, and material outside the repository are not covered by that
scan. Its shape-based rules catch representative opaque references and
home-directory paths; they cannot prove that every non-public reference is
absent. Test injections use synthetic content in a code comment,
decision record, and home path. Scanner errors fail the check. Diagnostics
identify a repository-relative file and rule class without printing matched
content.

Before publication, a human also reviews source and test data, branch names,
commit messages, PR titles and bodies, uploaded logs, and generated artifacts.
The tracked-tree check cannot certify those surfaces or remove historical
objects. Public change descriptions state the resulting behavior and its
verification, without private context or session narrative.

# Security policy

## Reporting a vulnerability

Please do not report security problems in public issues, pull requests or discussions.

Report them privately through GitHub: on the repository's **Security** tab, choose
**Report a vulnerability**, or go straight to
<https://github.com/officewright/officewright/security/advisories/new>. The report and
the discussion stay private between you and the maintainer until a fix is published.

Include what you can of:

- the release or commit, and how you use Officewright: the Rust crates, the C library
  (from which language) or the command-line tool;
- what happens, and what someone could gain from it;
- code, commands or an input file that reproduce it. Attach a document only if you may
  share it with the maintainer; never attach confidential or personal documents.

## What counts

Officewright is meant to read documents from untrusted sources. These are security
problems:

- memory unsafety through the C library, such as a crash, an out-of-bounds read or
  write, or a use after free;
- a panic, a hang or unbounded memory use caused by an input file: library code must
  return an error instead;
- reading or writing files, or making network requests, that the caller did not ask for,
  for example by following an external reference in a document.

## Supported versions

Officewright is in pre-release (0.x). Fixes go into `main` and the next release; earlier
releases are not patched.

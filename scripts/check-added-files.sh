#!/usr/bin/env bash
# Refuses files that must not be committed: Python bytecode (*.pyc, *.pyo, __pycache__/),
# which records the absolute path of its source, and files that name a home directory,
# which give away whose machine they came from:
#   /home/<name>, /Users/<name>, \\wsl.localhost\<distribution>\home\<name>,
#   C:\Users\<name> and C:\Documents and Settings\<name>,
# with / or single or doubled backslashes, in text and binary files. A placeholder such as
# <name>, $USER, %USERNAME% or {user} is allowed: a name is made of characters that do not
# start a placeholder. Text compressed inside a file (DOCX, XLSX and PPTX parts, PDF
# streams) or stored as UTF-16 is not searched; scripts/corpus-import.py and
# scripts/import-public-docs.py check the packages they bring in.
#
#   scripts/check-added-files.sh           every tracked file, as it is in the working tree
#   scripts/check-added-files.sh --staged  the files the commit adds, changes or renames,
#                                          as they are staged (.githooks/pre-commit)
set -euo pipefail

# A character of a user name: not a separator, a quote, white space, a control character
# or one that placeholders and markup use; or a %XX escape of a URL.
char='([^]/\<>&$%{}"'"'"'`*?|:,;()=[:space:][:cntrl:][]|%[0-9A-Fa-f]{2})'
name="$char+"
# Windows user names may have spaces inside, not at either end.
windows_name="$char(($char| )*$char)?"
sep='(\\+|/)'
# The name ends at a separator, a quote, white space or the end of the line.
end="($sep|[\"'\`[:space:]]|\$)"
pattern="${sep}home$sep$name$end|/Users/$name$end"
pattern+="|[A-Za-z]:$sep([Uu][Ss][Ee][Rr][Ss]|[Dd]ocuments and [Ss]ettings)$sep$windows_name$end"

# Searches the files named on standard input, NUL-separated, for home directories.
# `xargs` keeps each command line within the system's limit. git grep exits with 1 when
# it finds nothing; a higher status is an error, which stops xargs with status 124.
search() {
	xargs -0 sh -c 'git --literal-pathspecs grep "$@"; [ $? -le 1 ] || exit 255' sh \
		"$@" -a -n -o -E -e "$pattern" --
}

case ${1-} in
--staged)
	names=$(git -c core.quotePath=false diff --cached --name-only --diff-filter=d)
	# A commit that only removes files adds nothing to check.
	if [ -z "$names" ]; then
		exit 0
	fi
	status=0
	paths=$(git diff --cached --name-only -z --diff-filter=d | search --cached) || status=$?
	;;
'')
	names=$(git -c core.quotePath=false ls-files)
	status=0
	paths=$(git ls-files -z | search) || status=$?
	;;
*)
	echo "usage: $0 [--staged]" >&2
	exit 2
	;;
esac
if [ "$status" -ne 0 ] && [ "$status" -ne 123 ]; then
	echo "check-added-files: git grep failed (status $status)" >&2
	exit 1
fi

failed=
bytecode=$(printf '%s\n' "$names" | grep -E '(^|/)__pycache__/|\.py[co]$' || true)
if [ -n "$bytecode" ]; then
	echo "check-added-files: Python bytecode must not be committed:" >&2
	printf '%s\n' "$bytecode" | sed 's/^/  /' >&2
	failed=1
fi
if [ -n "$paths" ]; then
	echo "check-added-files: these files name a home directory; write <name> for the user:" >&2
	printf '%s\n' "$paths" | sed 's/^/  /' >&2
	failed=1
fi
if [ -n "$failed" ]; then
	exit 1
fi

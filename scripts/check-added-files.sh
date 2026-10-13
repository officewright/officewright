#!/usr/bin/env bash
# Refuses files that must not be committed: Python bytecode (*.pyc, *.pyo, __pycache__/),
# which records the absolute path of its source, and files that name a home directory,
# /home/<name>/, /Users/<name>/ or C:\Users\<name>\ (also with / or doubled backslashes),
# which give away whose machine they came from. A placeholder such as <name> is allowed.
#
#   scripts/check-added-files.sh           every tracked file, as it is in the working tree
#   scripts/check-added-files.sh --staged  the files added or changed in the index, as
#                                          they will be committed (.githooks/pre-commit)
set -euo pipefail

# A user name: letters, digits, dot, underscore, hyphen; Windows names may have spaces.
home='/home/[A-Za-z0-9._-]+/|/Users/[A-Za-z0-9._-]+/'
windows='[A-Za-z]:(\\+|/)[Uu][Ss][Ee][Rr][Ss](\\+|/)[A-Za-z0-9._ -]+(\\+|/)'
pattern="$home|$windows"

case ${1-} in
--staged)
	files=()
	while IFS= read -r -d '' file; do
		files+=("$file")
	done < <(git diff --cached --name-only -z --diff-filter=ACMR)
	if [ ${#files[@]} -eq 0 ]; then
		exit 0
	fi
	names=$(printf '%s\n' "${files[@]}")
	grep_args=(--cached)
	;;
'')
	names=$(git ls-files)
	files=()
	grep_args=()
	;;
*)
	echo "usage: $0 [--staged]" >&2
	exit 2
	;;
esac

failed=
bytecode=$(printf '%s\n' "$names" | grep -E '(^|/)__pycache__/|\.py[co]$' || true)
if [ -n "$bytecode" ]; then
	echo "check-added-files: Python bytecode must not be committed:" >&2
	printf '%s\n' "$bytecode" | sed 's/^/  /' >&2
	failed=1
fi
# -a searches binary files too; -o prints only the path found, never a binary line.
paths=$(git grep "${grep_args[@]}" -a -n -o -E -e "$pattern" -- "${files[@]}" || true)
if [ -n "$paths" ]; then
	echo "check-added-files: these files name a home directory; write <name> for the user:" >&2
	printf '%s\n' "$paths" | sed 's/^/  /' >&2
	failed=1
fi
if [ -n "$failed" ]; then
	exit 1
fi

#!/usr/bin/env bash
# Refuses files that must not be committed: Python bytecode (*.pyc, *.pyo, __pycache__/),
# which records the absolute path of its source, and files that name a home directory,
# which give away whose machine they came from:
#   /home/<name>, \\wsl.localhost\<distribution>\home\<name>, /Users/<name>,
#   C:\Users\<name> and C:\Documents and Settings\<name>,
# with / or single or doubled backslashes, in text and binary files; Users and Documents and
# Settings in any letter case, as macOS and Windows read them, home in lower case, as Linux
# does. A placeholder such as <name>, $USER, %USERNAME% or {user} is allowed. Text compressed inside a file (DOCX, XLSX and PPTX parts, PDF
# streams) or stored as UTF-16 is not searched; only the documents brought in through
# scripts/corpus-import.py and scripts/import-public-docs.py are checked there.
#
#   scripts/check-added-files.sh           every tracked file, as it is in the working tree
#   scripts/check-added-files.sh --staged  the files the commit adds, changes or renames,
#                                          as they are staged (.githooks/pre-commit)
set -euo pipefail

# The characters that end a user name: separators, quotes, white space, control
# characters, those no system allows in a user name (Windows refuses "/\[]:;|=,+*?<>, and
# Linux and macOS names are letters, digits, ".", "_" and "-") and those a placeholder or
# markup starts with ($, %, {, <, &). A name is the run of other characters before one of
# them, or before the end of the line; %XX escapes of a URL count as characters.
others=']/\<>&$%{}"'"'"'`*?|:,;()=[:space:][:cntrl:]['
char="([^$others]|%[0-9A-Fa-f]{2})"
name="$char+"
# Windows user names may have spaces inside, not at either end.
windows_name="$char(($char| )*$char)?"
sep='(\\+|/)'
end="($sep|[$others]|\$)"

# `word` with each letter in either case.
any_case() {
	local word=$1 out='' letter i
	for ((i = 0; i < ${#word}; i++)); do
		letter=${word:i:1}
		case $letter in
		[A-Za-z]) out+="[$(printf %s "$letter" | tr '[:lower:]' '[:upper:]')$(printf %s "$letter" | tr '[:upper:]' '[:lower:]')]" ;;
		*) out+=$letter ;;
		esac
	done
	printf %s "$out"
}
users=$(any_case Users)
documents=$(any_case 'Documents and Settings')
pattern="${sep}home$sep$name$end|$sep$users$sep$name$end"
pattern+="|[A-Za-z]:$sep($users|$documents)$sep$windows_name$end"

# Searches the files named on standard input, NUL-separated, for home directories.
# `xargs` keeps each command line within the system's limit. git grep exits with 1 when
# it finds nothing; a higher status is an error, which stops xargs with status 124.
search() {
	xargs -0 sh -c 'git --literal-pathspecs grep "$@"; [ $? -le 1 ] || exit 255' sh \
		"$@" -a -n -o -E -e "$pattern" --
}

# The files to check, NUL-separated, so that any file name is read as it is.
case ${1-} in
--staged)
	files() { git diff --cached --name-only -z --diff-filter=d; }
	cached=--cached
	# A commit that only removes files adds nothing to check.
	if [ "$(files | head -c 1 | wc -c)" -eq 0 ]; then
		exit 0
	fi
	;;
'')
	files() { git ls-files -z; }
	cached=
	;;
*)
	echo "usage: $0 [--staged]" >&2
	exit 2
	;;
esac
status=0
paths=$(files | search ${cached:+"$cached"}) || status=$?
if [ "$status" -ne 0 ] && [ "$status" -ne 123 ]; then
	echo "check-added-files: git grep failed (status $status)" >&2
	exit 1
fi

failed=
bytecode=
while IFS= read -r -d '' file; do
	case $file in
	__pycache__/* | */__pycache__/* | *.pyc | *.pyo) bytecode+="  $file"$'\n' ;;
	esac
done < <(files)
if [ -n "$bytecode" ]; then
	echo "check-added-files: Python bytecode must not be committed:" >&2
	printf '%s' "$bytecode" >&2
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

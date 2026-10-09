#!/usr/bin/env bash
# ------------------------------------------------------------------------------
# scripts/lint/check-real-identifiers.sh — block real-world identifiers (KYO-662)
#
# A customer's real GCP project id and a real service-account identity reached
# public `main` across four merged PRs and needed a history rewrite (see
# docs/standards/security/no-real-world-identifiers-in-a-public-repo.md,
# KYO-619/KYO-643). Per-PR human review caught it twice and it still got
# through, because two branches were in flight concurrently: one sanitised the
# identifiers while the other pasted them back in at new locations. This lint
# is the gate that catches it mechanically, at every point the material could
# be pasted in — a file (pre-commit, CI full-tree) or a commit message
# (commit-msg hook, CI over the PR's commits) — rather than relying on a human
# to remember to grep.
#
# Five rules, all BLOCKING (no advisory/WARN tier, no escape-hatch comment —
# see "Path exceptions" below for the one form of exception that exists):
#
#   gcp-service-account     — an email-shaped address whose domain ends in
#                              "iam.gserviceaccount.com" (split up here so
#                              this very sentence doesn't match its own rule).
#   email-address            — any other email-shaped string whose domain is
#                              not in the ALLOWED_DOMAINS policy list below.
#   public-ipv4              — a dotted quad that is not in a private,
#                              loopback, link-local, multicast, reserved, or
#                              RFC 5737 documentation range.
#   credential-token          — shapes of live API tokens (Slack, GitHub,
#                              AWS, OpenAI-style, Google OAuth, a bare
#                              Bearer header) that should never be committed
#                              even as "just a test value" — a real-looking
#                              token is exactly the kind of thing that gets
#                              copy-pasted from a real reproduction.
#   denylisted-identifier     — a salted-hash match against
#                              scripts/lint/real-identifiers-denylist.txt.
#                              This is the rule that exists because a GCP
#                              project id or a company name has no
#                              distinguishing shape at all — it looks like
#                              any other kebab-case token, so no regex can
#                              find it. See that file's own header for the
#                              full hashing contract.
#
# Modes (mutually exclusive):
#
#   check-real-identifiers.sh                    full tracked tree (CI)
#   check-real-identifiers.sh <file>...           only the listed files
#                                                  (pre-commit, staged paths)
#   check-real-identifiers.sh --message-file F    scan commit message file F
#                                                  (commit-msg hook: F is $1)
#   check-real-identifiers.sh --messages <range>  scan `git log --format=%B
#                                                  <range>` output (CI, over
#                                                  the PR's own commits)
#
# In full-tree/explicit-path mode, findings are reported as `path:line: rule
# <rule-id>`. In either message mode, the synthetic path `commit-message` is
# used instead, since a commit message has no file path of its own.
#
# Path exceptions (file modes only — never applies to a commit message,
# since "commit-message" is not a real path any glob here is written
# against): scripts/lint/real-identifiers-allow.txt, one
# `<rule-id>:<path-glob>:<justification>` entry per line. Mirrors the shape
# of trivy-secret.yaml's allow-rules, kept as a separate file per KYO-662
# because this linter's rule ids are its own. This is for files that
# deliberately contain fake, structurally-real-looking material for tests —
# never for a real identifier.
#
# IMPORTANT — never print a matched value, for any rule, under any
# circumstance. CI logs on a public repo are themselves public, so echoing
# the match would leak exactly what this gate exists to protect. The
# location (`path:line`) is always enough for the author to open the file
# and see it themselves. Do NOT "helpfully" add the matched text to a
# finding or an error message in a future change to this script.
#
# Tokenisation contract for the denylisted-identifier rule (must match
# exactly what generated the hashes in real-identifiers-denylist.txt, or the
# hashes never fire): from each line, extract every token matching
#   [A-Za-z0-9][A-Za-z0-9._@-]{2,62}[A-Za-z0-9]
# then additionally emit every non-empty segment obtained by splitting that
# token on `[._@-]`. Lowercase everything, then hash each candidate as
# sha256_hex(salt + ":" + candidate) and check membership. Emitting segments
# as well as whole tokens is deliberate: a denylist entry for a bare company
# name also catches `<name>-dev`, `<name>_prod`, and `user@<name>`.
#
# Fail-closed: any input this script cannot read or parse (a target file it
# cannot open, a malformed denylist/allow-list entry, `git log`/`git
# ls-files` failing) is a non-zero exit — never treated as "no violations
# found here", which would be indistinguishable from a clean scan.
#
# Environment:
#   REAL_IDENTIFIERS_DENYLIST   override the denylist file path. Used by
#                               check-real-identifiers-test.sh to point at a
#                               throwaway denylist built over a synthetic
#                               value, so the self-test never needs the real
#                               (private) plaintext list.
#
# Exit codes: 0 clean, 1 violations found (or an input could not be read —
#             see "Fail-closed" above), 2 usage error.
#
# Bash driver + a single embedded Perl pass (Perl, not awk, because this
# rule set needs SHA-256 hashing (Digest::SHA, core) and Perl's regex engine
# gives us lookaround for clean word-boundary handling on the IPv4 rule;
# awk has neither). No Rust toolchain required.
# ------------------------------------------------------------------------------

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
ALLOW_FILE="$SCRIPT_DIR/real-identifiers-allow.txt"
DENYLIST_FILE="${REAL_IDENTIFIERS_DENYLIST:-$SCRIPT_DIR/real-identifiers-denylist.txt}"

usage() {
    cat >&2 <<'USAGE'
usage:
  check-real-identifiers.sh                    scan the full tracked tree
  check-real-identifiers.sh <file>...           scan only the listed files
  check-real-identifiers.sh --message-file F    scan a commit-message file
  check-real-identifiers.sh --messages <range>  scan `git log --format=%B <range>`
USAGE
}

MODE="paths"
MESSAGE_FILE=""
MESSAGES_RANGE=""
declare -a EXPLICIT_PATHS=()

while [ "$#" -gt 0 ]; do
    case "$1" in
        --message-file)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --message-file requires an argument" >&2
                usage
                exit 2
            fi
            if [ "$MODE" != "paths" ] || [ "${#EXPLICIT_PATHS[@]}" -gt 0 ]; then
                echo "ERROR: --message-file cannot be combined with other modes" >&2
                exit 2
            fi
            MODE="message-file"
            MESSAGE_FILE="$2"
            shift 2
            ;;
        --messages)
            if [ "$#" -lt 2 ]; then
                echo "ERROR: --messages requires an argument" >&2
                usage
                exit 2
            fi
            if [ "$MODE" != "paths" ] || [ "${#EXPLICIT_PATHS[@]}" -gt 0 ]; then
                echo "ERROR: --messages cannot be combined with other modes" >&2
                exit 2
            fi
            MODE="messages"
            MESSAGES_RANGE="$2"
            shift 2
            ;;
        -h|--help)
            usage
            exit 0
            ;;
        --)
            shift
            while [ "$#" -gt 0 ]; do
                EXPLICIT_PATHS+=("$1")
                shift
            done
            ;;
        -*)
            echo "ERROR: unknown option: $1" >&2
            usage
            exit 2
            ;;
        *)
            if [ "$MODE" = "message-file" ] || [ "$MODE" = "messages" ]; then
                echo "ERROR: cannot combine a path argument with --message-file/--messages" >&2
                exit 2
            fi
            EXPLICIT_PATHS+=("$1")
            shift
            ;;
    esac
done

if [ ! -f "$DENYLIST_FILE" ]; then
    echo "ERROR: denylist file not found: $DENYLIST_FILE" >&2
    echo "       (set REAL_IDENTIFIERS_DENYLIST to override the path)" >&2
    exit 1
fi

declare -a TMP_FILES=()
cleanup() {
    rm -f "${TMP_FILES[@]}"
}
trap cleanup EXIT

MANIFEST="$(mktemp)"
TMP_FILES+=("$MANIFEST")

# ------------------------------------------------------------------------------
# Build the manifest: one `label<TAB>on-disk-path` line per thing to scan.
# `label` is what gets printed in findings; `on-disk-path` is where the Perl
# pass actually reads the bytes from. Unifying every mode into this one shape
# keeps the scanning logic below mode-agnostic.
# ------------------------------------------------------------------------------
case "$MODE" in
    message-file)
        if [ ! -f "$MESSAGE_FILE" ]; then
            echo "ERROR: message file not found: $MESSAGE_FILE" >&2
            exit 2
        fi
        printf 'commit-message\t%s\n' "$MESSAGE_FILE" > "$MANIFEST"
        ;;
    messages)
        MSG_TMP="$(mktemp)"
        TMP_FILES+=("$MSG_TMP")
        ERR_TMP="$(mktemp)"
        TMP_FILES+=("$ERR_TMP")
        # Written directly to a real file (not captured via `$(...)` or a
        # process substitution) so its exit status is checked the ordinary
        # way — see docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md
        # on why a process substitution's exit status must never be the
        # thing standing between a real failure and a silent empty result.
        if ! git -C "$REPO_ROOT" log --format=%B "$MESSAGES_RANGE" > "$MSG_TMP" 2>"$ERR_TMP"; then
            echo "ERROR: git log --format=%B '$MESSAGES_RANGE' failed:" >&2
            cat "$ERR_TMP" >&2
            exit 2
        fi
        printf 'commit-message\t%s\n' "$MSG_TMP" > "$MANIFEST"
        ;;
    paths)
        : > "$MANIFEST"
        if [ "${#EXPLICIT_PATHS[@]}" -gt 0 ]; then
            for p in "${EXPLICIT_PATHS[@]}"; do
                if [ -f "$p" ]; then
                    printf '%s\t%s\n' "$p" "$p" >> "$MANIFEST"
                elif [ -f "$REPO_ROOT/$p" ]; then
                    printf '%s\t%s\n' "$p" "$REPO_ROOT/$p" >> "$MANIFEST"
                fi
                # A path that resolves to neither has been deleted by the
                # commit under scan (pre-commit passes staged paths
                # including deletions) — a deleted file can't introduce a
                # new violation, so it is skipped silently, matching
                # check-server-fns.sh's handling of the same case.
            done
        else
            LIST_TMP="$(mktemp)"
            TMP_FILES+=("$LIST_TMP")
            if ! git -C "$REPO_ROOT" ls-files -z > "$LIST_TMP"; then
                echo "ERROR: git ls-files failed" >&2
                exit 1
            fi
            while IFS= read -r -d '' relpath; do
                printf '%s\t%s\n' "$relpath" "$REPO_ROOT/$relpath" >> "$MANIFEST"
            done < "$LIST_TMP"
        fi
        ;;
esac

PERL_PROGRAM="$(mktemp)"
TMP_FILES+=("$PERL_PROGRAM")

cat > "$PERL_PROGRAM" <<'PERL_EOF'
use strict;
use warnings;
use Digest::SHA qw(sha256_hex);

my $manifest = $ARGV[0] or die "check-real-identifiers: no manifest given\n";
my $allow_file     = $ENV{RI_ALLOW_FILE};
my $denylist_file  = $ENV{RI_DENYLIST_FILE} // die "check-real-identifiers: RI_DENYLIST_FILE not set\n";

# ------------------------------------------------------------------------------
# Glob -> anchored regex, for the path exceptions file. `*` matches any run
# of characters, `?` matches exactly one; everything else is literal.
# ------------------------------------------------------------------------------
sub glob_to_regex {
    my ($glob) = @_;
    my $re = quotemeta($glob);
    $re =~ s/\\\*/.*/g;
    $re =~ s/\\\?/./g;
    return qr/^$re$/;
}

# ------------------------------------------------------------------------------
# Load path exceptions. Malformed entries fail closed (die) rather than
# being silently skipped — a typo'd exception line should never quietly
# turn into "no exceptions at all" OR "an exception nobody can explain";
# either way a human needs to see it and fix it.
# ------------------------------------------------------------------------------
my %allow;
if (defined $allow_file && -f $allow_file) {
    open(my $fh, '<', $allow_file) or die "check-real-identifiers: cannot open allow file $allow_file: $!\n";
    while (my $line = <$fh>) {
        chomp $line;
        next if $line =~ /^\s*$/;
        next if $line =~ /^\s*#/;
        my ($rule, $glob, $just) = split(/:/, $line, 3);
        if (!defined $just || $just eq '') {
            die "check-real-identifiers: malformed allow-file entry (need <rule-id>:<path-glob>:<justification>): $line\n";
        }
        push @{ $allow{$rule} }, glob_to_regex($glob);
    }
    close $fh;
}

sub is_allowed {
    my ($rule, $label) = @_;
    return 0 unless exists $allow{$rule};
    for my $re (@{ $allow{$rule} }) {
        return 1 if $label =~ $re;
    }
    return 0;
}

# ------------------------------------------------------------------------------
# Load the denylist. See real-identifiers-denylist.txt's own header for the
# full hashing contract this must match exactly. Any parse failure here
# fails closed (die -> non-zero exit) rather than degrading to "denylist
# rule never fires" — that failure mode is indistinguishable from a clean
# scan, which is exactly the shape
# docs/standards/error-handling/empty-on-failure-must-not-look-like-a-real-result.md
# warns about.
# ------------------------------------------------------------------------------
my $salt;
my %denyhash;
{
    open(my $fh, '<', $denylist_file) or die "check-real-identifiers: cannot open denylist file $denylist_file: $!\n";
    while (my $line = <$fh>) {
        chomp $line;
        next if $line =~ /^\s*$/;
        if ($line =~ /^\s*#\s*salt=([0-9a-fA-F]+)\s*$/) {
            die "check-real-identifiers: denylist file has more than one salt= line\n" if defined $salt;
            $salt = lc $1;
            next;
        }
        next if $line =~ /^\s*#/;
        if ($line !~ /^[0-9a-fA-F]{64}$/) {
            die "check-real-identifiers: malformed denylist entry (expected a 64-hex-char sha256 hash): $line\n";
        }
        $denyhash{ lc $line } = 1;
    }
    close $fh;
    die "check-real-identifiers: denylist file has no '# salt=' header line\n" unless defined $salt;
}

# ------------------------------------------------------------------------------
# Policy constant: domains that are not customer/production identifiers.
# Each entry carries the one-line reason it's here — this array IS the
# policy, not a cache of one, so a new legitimate domain is a one-line PR
# to this file, not a workaround.
# ------------------------------------------------------------------------------
my %ALLOWED_DOMAINS = (
    'test.local'         => 'synthetic fixture domain used across the test suite',
    'contract-test.local' => 'synthetic fixture domain used by the SSR contract test suite',
    'example.com'        => 'IANA-reserved for documentation (RFC 2606) — never a real registration',
    'kyomi.ai'            => 'first-party production domain',
    'kyomi.dev'           => 'first-party development domain',
    'kyomi.invalid'       => 'first-party test domain under the .invalid TLD, reserved for this purpose (RFC 2606)',
    'fcm.googleapis.com'  => 'Firebase Cloud Messaging — a public third-party API endpoint this app calls, not a customer identifier',
    'company.com'         => 'common placeholder domain used in fixture/example data',
    'evil.com'            => 'deliberately-named placeholder domain used in adversarial/security test fixtures',
    'email.com'           => 'common placeholder domain used in fixture/example data',
    'test.com'            => 'common placeholder domain used in fixture/example data',
    'db.internal'         => 'synthetic internal-only hostname used in fixture data, not a public domain',
    # KYO-662 measured false positives, handled here rather than by
    # tightening the shared email regex (see EMAIL_RE below), because both
    # are single, specific, already-known strings rather than a general
    # shape worth a regex exception:
    'b.iam'               => 'artifact of a deliberately truncated fake service-account JSON fixture ("a@b.iam") in credential_service.rs — not a real domain, "iam" just happens to look like a TLD',
    '2x.png'              => 'artifact of a retina asset reference ("icon@2x.png") being mistaken for an email address by the generic email shape — not a domain at all. Residual gap: an "@3x.png" reference elsewhere would need its own entry the same way.',
);

# ------------------------------------------------------------------------------
# Policy constant: IPv4 ranges that are not "a real, potentially
# identifying, public address" — private/loopback/link-local/multicast/
# reserved ranges, plus the RFC 5737 documentation ranges, which exist
# specifically so examples never need a real address.
# ------------------------------------------------------------------------------
my @PRIVATE_CIDRS = (
    ['0.0.0.0',         32],  # "this host on this network" (RFC 791) — single address
    ['10.0.0.0',         8],
    ['127.0.0.0',        8],  # loopback
    ['169.254.0.0',     16],  # link-local
    ['172.16.0.0',      12],
    ['192.0.2.0',       24],  # RFC 5737 TEST-NET-1 (documentation)
    ['192.168.0.0',     16],
    ['198.51.100.0',    24],  # RFC 5737 TEST-NET-2 (documentation)
    ['203.0.113.0',     24],  # RFC 5737 TEST-NET-3 (documentation)
    ['224.0.0.0',        4],  # multicast
    ['240.0.0.0',        4],  # reserved
    ['255.255.255.255', 32],  # broadcast — single address
);

# ------------------------------------------------------------------------------
# Policy constant, parallel to ALLOWED_DOMAINS above: specific public IPv4
# addresses that are not a real customer's address — either a third party's
# own well-known public infrastructure (analogous to fcm.googleapis.com in
# ALLOWED_DOMAINS), or a de facto "obviously an example" address used
# repeatedly across this repo's tests and doc comments the same way
# example.com is for domains. Unlike RFC 5737 (192.0.2.0/24 etc.), none of
# these are formally reserved for documentation use — that mismatch is
# exactly why they need to be named here individually rather than covered by
# a CIDR range.
# ------------------------------------------------------------------------------
my %ALLOWED_IPS = (
    '1.2.3.4'   => 'de facto "obviously an example" placeholder address used throughout this repo\'s tests and doc comments',
    '5.6.7.8'   => 'de facto "obviously an example" placeholder address, always paired with 1.2.3.4 in the same fixtures',
    '8.8.8.8'   => 'Google Public DNS — well-known third-party public infrastructure, not a customer identifier',
    '210.0.0.1' => 'synthetic placeholder address used in error-sanitizer redaction tests',
    # KYO-662 measured false positive, handled here rather than by
    # tightening the shared IP regex: "RFC 6749 §4.1.2.1" in
    # oauth_popup.rs is a spec section number, not an IP address at all —
    # it only has the shape of one.
    '4.1.2.1'   => 'artifact of an RFC section-number citation ("RFC 6749 §4.1.2.1") in oauth_popup.rs being mistaken for an IPv4 address by the generic dotted-quad shape — not an address at all',
);

sub ip_to_int {
    my ($ip) = @_;
    my @o = split(/\./, $ip);
    return undef unless @o == 4;
    for my $part (@o) {
        return undef if $part !~ /^\d{1,3}$/;
        return undef if $part > 255;
    }
    return (($o[0] << 24) | ($o[1] << 16) | ($o[2] << 8) | $o[3]);
}

sub ip_is_excluded {
    my ($ipi) = @_;
    for my $r (@PRIVATE_CIDRS) {
        my ($net, $bits) = @$r;
        my $neti = ip_to_int($net);
        my $mask = $bits == 0 ? 0 : ((0xFFFFFFFF << (32 - $bits)) & 0xFFFFFFFF);
        return 1 if (($ipi & $mask) == ($neti & $mask));
    }
    return 0;
}

# ------------------------------------------------------------------------------
# Rule regexes. These shapes are the ticket spec verbatim (KYO-662) — do not
# loosen or tighten them ad hoc; a false positive on a synthetic fixture
# belongs in real-identifiers-allow.txt, not a regex edit here.
# ------------------------------------------------------------------------------
my $GSA_RE   = qr/[A-Za-z0-9._%+-]+\@[A-Za-z0-9.-]+\.iam\.gserviceaccount\.com/;
my $EMAIL_RE = qr/[A-Za-z0-9._%+-]+\@[A-Za-z0-9.-]+\.[A-Za-z]{2,}/;
my $IP_RE    = qr/(?<![0-9.])(?:\d{1,3}\.){3}\d{1,3}(?![0-9.])/;
my $TOKEN_RE = qr/[A-Za-z0-9][A-Za-z0-9._@-]{2,62}[A-Za-z0-9]/;

my @CRED_PATTERNS = (
    qr/ya29\.[0-9A-Za-z_-]{20,}/,
    qr/sk-[A-Za-z0-9]{20,}/,
    qr/ghp_[A-Za-z0-9]{36}/,
    qr/gho_[A-Za-z0-9]{36}/,
    qr/github_pat_[A-Za-z0-9_]{50,}/,
    qr/xox[baprs]-[0-9A-Za-z-]{10,}/,
    qr/AKIA[0-9A-Z]{16}/,
    qr/Bearer [A-Za-z0-9._~+\/=-]{30,}/,
);

my %REMEDIATE = (
    'gcp-service-account'   => 'replace the GCP service-account address with a synthetic placeholder (e.g. test-service-account@test-project), or add a path exception to scripts/lint/real-identifiers-allow.txt if this is a deliberate test fixture',
    'email-address'         => 'replace the email address with a synthetic placeholder, or (only if the domain itself is non-identifying, e.g. a first-party or documentation domain) add it to ALLOWED_DOMAINS in scripts/lint/check-real-identifiers.sh',
    'public-ipv4'           => 'replace the public IPv4 address with an RFC 5737 documentation address (192.0.2.0/24, 198.51.100.0/24, or 203.0.113.0/24), or a private-range address',
    'credential-token'      => 'remove the credential-shaped token; if it is a deliberately fake test fixture, add a path exception to scripts/lint/real-identifiers-allow.txt',
    'denylisted-identifier' => 'remove the forbidden identifier — see scripts/lint/real-identifiers-denylist.txt for how this list works and who to ask before changing it',
);

# ------------------------------------------------------------------------------
# Scan.
# ------------------------------------------------------------------------------
my $violation_count = 0;
my $read_errors = 0;

open(my $mfh, '<', $manifest) or die "check-real-identifiers: cannot open manifest $manifest: $!\n";
while (my $entry = <$mfh>) {
    chomp $entry;
    next if $entry eq '';
    my ($label, $path) = split(/\t/, $entry, 2);
    die "check-real-identifiers: malformed manifest entry: $entry\n" unless defined $path;

    my $fh;
    if (!open($fh, '<:raw', $path)) {
        # Fail closed: a target this script cannot read is not "no
        # findings here", it is "this check did not run" — report and
        # count it as a failure of the run, not a clean file.
        print STDERR "$label: could not be read ($!)\n";
        $read_errors++;
        next;
    }
    local $/ = undef;
    my $content = <$fh>;
    close $fh;
    $content = '' unless defined $content;

    if (index($content, "\0") >= 0) {
        # Binary file — this lint scans tracked *text*, matching every
        # other lint script in this repo. Skipping is not a fail-open risk
        # here: the point of every rule below is to catch material a human
        # typed or pasted, which does not happen inside a binary asset.
        next;
    }

    my @lines = split(/\n/, $content, -1);
    my $lineno = 0;
    for my $line (@lines) {
        $lineno++;
        $line =~ s/\r\z//;
        next if $line eq '';

        my %fired;

        while ($line =~ /$GSA_RE/g) {
            $fired{'gcp-service-account'} = 1;
        }

        while ($line =~ /($EMAIL_RE)/g) {
            my $m = $1;
            my ($domain) = $m =~ /\@(.+)\z/;
            next unless defined $domain;
            $domain = lc $domain;
            next if $domain =~ /\.iam\.gserviceaccount\.com\z/;
            next if exists $ALLOWED_DOMAINS{$domain};
            $fired{'email-address'} = 1;
        }

        while ($line =~ /($IP_RE)/g) {
            my $ip = $1;
            next if exists $ALLOWED_IPS{$ip};
            my $ipi = ip_to_int($ip);
            next unless defined $ipi;
            next if ip_is_excluded($ipi);
            $fired{'public-ipv4'} = 1;
        }

        for my $re (@CRED_PATTERNS) {
            if ($line =~ /$re/) {
                $fired{'credential-token'} = 1;
                last;
            }
        }

        my %candidates;
        while ($line =~ /($TOKEN_RE)/g) {
            my $tok = lc $1;
            $candidates{$tok} = 1;
            for my $seg (split(/[._@-]/, $tok)) {
                $candidates{$seg} = 1 if $seg ne '';
            }
        }
        for my $cand (keys %candidates) {
            my $h = sha256_hex("$salt:$cand");
            if (exists $denyhash{$h}) {
                $fired{'denylisted-identifier'} = 1;
                last;
            }
        }

        for my $rule (sort keys %fired) {
            next if is_allowed($rule, $label);
            # Never print $line, $m, $ip, $tok, or any other matched
            # substring here — see this script's header. The location is
            # always enough; the value is exactly what this gate exists to
            # keep out of a public CI log.
            print "$label:$lineno: rule $rule\n";
            print "  fix: $REMEDIATE{$rule}\n";
            $violation_count++;
        }
    }
}
close $mfh;

if ($read_errors > 0) {
    print STDERR "check-real-identifiers: $read_errors target(s) could not be read — treating as a failed run\n";
    exit 1;
}

exit($violation_count > 0 ? 1 : 0);
PERL_EOF

export RI_ALLOW_FILE="$ALLOW_FILE"
export RI_DENYLIST_FILE="$DENYLIST_FILE"

set +e
perl "$PERL_PROGRAM" "$MANIFEST"
rc=$?
set -e

# Normalize to the documented contract (0 clean, 1 violations-or-failure, 2
# usage error — usage errors are all handled above, before Perl ever runs).
# A `die` inside the embedded Perl program exits 255, not 1; collapse
# anything non-zero to 1 so a fail-closed internal error is never
# indistinguishable from "usage error" (2) or, worse, silently coerced to a
# clean 0 by a caller that only checks "was it exactly 1".
if [ "$rc" -ne 0 ]; then
    exit 1
fi
exit 0

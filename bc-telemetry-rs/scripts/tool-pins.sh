# The versions of the programs this workspace's tests run that are not Rust crates,
# each spelled here and nowhere else. Assignments only: sourced by
# scripts/check-test-host-programs.sh and scripts/fetch-grafana.sh, which
# scripts/verify.sh runs.
#
# A pin is not a floor, and each is spelled once, because a version spelled in
# several places drifts until the places disagree about what runs:
#
#   floor — the oldest version whose behavior the code relies on;
#   pin   — the one version the tests run on, here and on the CI runner alike.
#
# Moving a pin is one edit here and a re-run of verify.sh; what a pin fixes
# beside it, and which moves when it does, is listed with it below.

# shellcheck shell=sh
# shellcheck disable=SC2034  # read by the sourcing scripts, not within this file

# PostgreSQL. The tests run on this one major, here and on the CI runner, so the
# layout and the exporter are tested on one server version rather than whichever
# one a host happens to have. The programs are the host's own packages at that
# major — a Homebrew formula on a Mac, a Debian package on the CI runner
# (scripts/check-test-host-programs.sh names both) — so the patch release floats
# with the host's security updates. The floor is what the generated layout's SQL
# needs.
TOOL_PIN_POSTGRESQL_MAJOR=17
TOOL_FLOOR_POSTGRESQL_MAJOR=14

# Grafana, which the generator's integration tier runs from Grafana's own release
# tarball — never a package manager's, whose upgrade would move it. Its build
# number is part of every tarball's name, and each platform's hash is the one
# published beside its tarball (`<tarball>.sha256`).
TOOL_PIN_GRAFANA=13.2.3
TOOL_PIN_GRAFANA_BUILD=36482603486
TOOL_PIN_GRAFANA_SHA256_DARWIN_ARM64=248a51bcfacdb1ec642006cee46b4de44a34bcf41ddea88d96a4605e2e9d808c
TOOL_PIN_GRAFANA_SHA256_DARWIN_AMD64=59d08d3ef611a49239bf3b54cd1cb1b215611a3f6cd37bb6e6abe40dda181319
TOOL_PIN_GRAFANA_SHA256_LINUX_ARM64=a2a41b960ba4c25e83140484e48813730d3c81891a128439a2a9d2df6eb3840e
TOOL_PIN_GRAFANA_SHA256_LINUX_AMD64=6107ad27016296aac38e0d7ffa8753ab540b5541ad27e94790f771289d733235

# Grafana's PostgreSQL data source, which the tier tests through. Grafana 13 does not
# ship it in its tarball: unconfigured, it downloads the catalog's newest release at
# every start (`[plugins] preinstall`), so the data source under test would move on
# its own. scripts/fetch-grafana.sh fetches this release into the pinned Grafana's
# `plugins-bundled`, and the tier turns preinstall off, so a run fetches nothing.
# Each hash is the one the catalog publishes for that platform's package
# (https://grafana.com/api/plugins/grafana-postgresql-datasource/versions).
TOOL_PIN_GRAFANA_POSTGRESQL_DATASOURCE=13.0.4
TOOL_PIN_GRAFANA_POSTGRESQL_DATASOURCE_SHA256_DARWIN_ARM64=9272619ef5be517d4628f3e6f7b8ce13e976467cfea52a6db69b82d4b64f768c
TOOL_PIN_GRAFANA_POSTGRESQL_DATASOURCE_SHA256_DARWIN_AMD64=e8a951d5f0bfe88ac7e2659a767a21fe50c79665e58f4608c1a4acfdc70f4f8d
TOOL_PIN_GRAFANA_POSTGRESQL_DATASOURCE_SHA256_LINUX_ARM64=7b3af34998cbb7ef7854991f7103d6c4004c42c9a8bd405f90833b11e652b083
TOOL_PIN_GRAFANA_POSTGRESQL_DATASOURCE_SHA256_LINUX_AMD64=d34f44bd13213502de4394955fcc170d70fb822531c181bff0179f216c35a0e0

# What the pinned Grafana fixes beside it, which moves when it does:
#   - the newest dashboard schemaVersion it migrates to (its LATEST_VERSION), which the
#     dashboard model writes (crates/dashboard/src/lib.rs);
#   - the Foundation SDK release for it — past Grafana 11 the SDK publishes one line,
#     `latest`, "best suited for Grafana >= 12" — which the TypeScript proof installs
#     (crates/gen/tests/typescript/package.json);
#   - that SDK release's dashboard JSON Schema, vendored as the dashboards' oracle
#     (crates/dashboard/schema/dashboard.jsonschema.json).
TOOL_PIN_GRAFANA_DASHBOARD_SCHEMA_VERSION=42
TOOL_PIN_GRAFANA_FOUNDATION_SDK=0.0.20
TOOL_PIN_GRAFANA_DASHBOARD_SCHEMA_SHA256=67451cd5b288f97e3f68eccd52ffa4f75151fc87c669b8cd93c051056b294265

# Node.js, for the TypeScript proof: a floor only, the major the CI runner's Debian 13
# carries. The proof type-checks and validates a document; nothing it runs varies
# within a major.
TOOL_FLOOR_NODE_MAJOR=20

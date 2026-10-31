"""Security contracts of the fork's CI (decrypted/dbflux); no credentials or runner required.

The fork merges upstream's workflows, and a merge can quietly undo the release hardening: put
the GPG key back into a job that compiles ~1300 crates, re-add `secrets: inherit`, unpin an
action or drop a repository guard. These tests parse the workflow YAML and fail when that
happens. They run in the fork's `checks` job (fork-build.yml, fork-release.yml) with
DBFLUX_REQUIRE_YAML=1, and locally with
`python3 -m unittest discover -s scripts -p 'test_*.py'`.
"""
import os
import pathlib
import re
import unittest

try:
    import yaml
except ImportError:  # pragma: no cover - depends on the machine
    yaml = None

ROOT = pathlib.Path(__file__).resolve().parents[1]
WORKFLOWS = ROOT / ".github/workflows"

UPSTREAM_GUARD = "github.repository == '0xErwin1/dbflux'"
FORK_GUARD = "github.repository == 'decrypted/dbflux'"
FORK_KEY_FINGERPRINT = "527F909CB6650DF3C3F7B410EF24E6D33BC0C1EC"
BUILD_CALL = "./.github/workflows/build.yml"

# Every workflow file, classified. A workflow merged in from upstream fails
# test_every_workflow_is_classified until someone decides where it belongs.
FORK = {"fork-build.yml", "fork-release.yml"}
REUSABLE = {"build.yml"}
# Publish upstream's release, tap, site or nightly: never run in the fork.
UPSTREAM_ONLY = {"release.yml", "nightly.yml", "web.yml"}
# Run only on upstream's main/dev events, publish nothing and skip cleanly without upstream's
# secrets; GitHub keeps tests/style disabled in the fork.
UPSTREAM_HARMLESS = {"installer.yml", "tests.yml", "style.yml"}
FORK_REACHABLE = FORK | REUSABLE

KEY_NAMES = re.compile(r"\bGPG_(?:PRIVATE_KEY|PASSPHRASE)\b")
SIGNING_SECRETS = re.compile(r"\b(?:GPG_PRIVATE_KEY|GPG_PASSPHRASE|MACOS_CERTIFICATE\w*|SIGNPATH_API_TOKEN)\b")
GPG_IMPORT = re.compile(r"\bgpg\b[^\n]*--import\b")
SIGNING_COMMAND = re.compile(r"\bgpg\b|\bdebsigs\b|\brpm\s+--addsign\b")
# Commands that compile code or install third-party tools. A job running any of them must never
# see the signing key.
BUILD_OR_INSTALL = re.compile(
    r"\b(?:cargo|rustup|rustc|make|cmake|npm|npx|pnpm|yarn|pip3?|go\s+(?:install|build|get|run)"
    r"|brew|apt-get|apt|dnf|yum|docker|nfpm|curl|wget)\b"
)
LOCK_GATE = re.compile(r"\bcargo\s+(?:metadata|fetch|tree|check|build)\b[^\n]*--locked\b")
UNTRUSTED_EXPRESSION = re.compile(r"\$\{\{\s*(?:inputs\.|github\.event\.|github\.head_ref\b)")
FULL_SHA = re.compile(r"^[^@\s]+@[0-9a-f]{40}$")
KEY_JOB_ACTIONS = {"actions/download-artifact", "actions/upload-artifact"}


def setUpModule():
    if yaml is None:
        if os.environ.get("DBFLUX_REQUIRE_YAML"):
            raise RuntimeError("PyYAML is required for the CI security contracts (apt-get install python3-yaml).")
        raise unittest.SkipTest("PyYAML is not installed; the CI security contracts need it.")


def load(name):
    data = yaml.safe_load((WORKFLOWS / name).read_text(encoding="utf-8"))
    # YAML 1.1 (PyYAML) reads the key `on` as the boolean True.
    if True in data:
        data["on"] = data.pop(True)
    return data


def strings(value):
    """Every string in a parsed YAML value, mapping keys included."""
    if isinstance(value, str):
        yield value
    elif isinstance(value, dict):
        for key, item in value.items():
            yield str(key)
            yield from strings(item)
    elif isinstance(value, list):
        for item in value:
            yield from strings(item)


def mentions(value, pattern):
    return any(pattern.search(text) for text in strings(value))


def script(step):
    """A step's run: script without full-line comments, continuation lines joined."""
    lines = [line for line in str(step.get("run", "")).splitlines() if not line.lstrip().startswith("#")]
    return "\n".join(lines).replace("\\\n", " ")


def steps(job):
    return job.get("steps") or []


def action(step):
    uses = step.get("uses", "")
    return uses.split("@", 1)[0] if uses else ""


def needs(job):
    value = job.get("needs") or []
    return [value] if isinstance(value, str) else list(value)


def holds_key(job):
    """The job can read the signing key or loads one into gpg."""
    return (
        job.get("secrets") == "inherit"  # every secret, the key included
        or mentions(job, KEY_NAMES)
        or any("import-gpg" in action(step) for step in steps(job))
        or any(GPG_IMPORT.search(script(step)) for step in steps(job))
    )


def builds_or_installs(job):
    """The job compiles code or runs tooling from outside the runner image."""
    if "uses" in job:  # a reusable workflow call: the build itself
        return True
    for step in steps(job):
        if BUILD_OR_INSTALL.search(script(step)):
            return True
        name = action(step)
        if name and name not in KEY_JOB_ACTIONS | {"actions/checkout"}:
            return True
    return False


def build_calls(workflow):
    return {name: job for name, job in workflow["jobs"].items() if job.get("uses") == BUILD_CALL}


class WorkflowInventoryTests(unittest.TestCase):
    def test_every_workflow_is_classified(self):
        present = {path.name for path in WORKFLOWS.iterdir() if path.suffix in (".yml", ".yaml")}
        known = FORK | REUSABLE | UPSTREAM_ONLY | UPSTREAM_HARMLESS
        self.assertEqual(
            present - known, set(),
            "New workflow file(s): add a repository guard if they publish or need upstream secrets, "
            "then classify them in scripts/test_workflow_security.py.",
        )
        self.assertEqual(known - present, set(), "A classified workflow file is gone; update the lists.")


class SigningKeyTests(unittest.TestCase):
    def test_fork_build_references_no_signing_secret(self):
        workflow = load("fork-build.yml")
        self.assertFalse(mentions(workflow, SIGNING_SECRETS), "fork-build.yml must not reference any signing secret")
        for name, job in workflow["jobs"].items():
            with self.subTest(job=name):
                self.assertNotIn("secrets", job)
                self.assertNotIn("environment", job)
                self.assertFalse(holds_key(job))

    def test_no_secrets_inherit_in_fork_workflows(self):
        for name in FORK:
            with self.subTest(workflow=name):
                for job_name, job in load(name)["jobs"].items():
                    self.assertNotEqual(job.get("secrets"), "inherit", f"{job_name} passes every secret")

    def test_fork_callers_build_unsigned_and_without_secrets(self):
        for name in FORK:
            with self.subTest(workflow=name):
                calls = build_calls(load(name))
                self.assertEqual(len(calls), 1, "expected one call of build.yml")
                for job in calls.values():
                    self.assertIs(job.get("with", {}).get("sign_in_build"), False)
                    self.assertIn(job.get("secrets"), (None, {}), "the build jobs compile code and get no secrets")

    def test_no_job_that_builds_or_installs_holds_the_key(self):
        for name in FORK:
            workflow = load(name)
            self.assertFalse(mentions(workflow.get("env", {}), KEY_NAMES), f"{name}: no key in the workflow env")
            for job_name, job in workflow["jobs"].items():
                if builds_or_installs(job):
                    with self.subTest(workflow=name, job=job_name):
                        self.assertFalse(holds_key(job), f"{job_name} builds or installs tools and holds the key")

    def test_only_the_sign_job_holds_the_key(self):
        workflow = load("fork-release.yml")
        jobs = workflow["jobs"]
        self.assertEqual({name for name, job in jobs.items() if holds_key(job)}, {"sign"})
        sign = jobs["sign"]
        environment = sign.get("environment")
        if isinstance(environment, dict):
            environment = environment.get("name")
        self.assertEqual(environment, "release")
        self.assertFalse(builds_or_installs(sign), "the sign job must not compile or install anything")
        self.assertNotIn("uses", sign)
        used = {action(step) for step in steps(sign) if "uses" in step}
        self.assertLessEqual(used, KEY_JOB_ACTIONS, "the sign job uses only the artifact actions")
        self.assertFalse(any("cargo" in script(step) for step in steps(sign)))
        self.assertNotIn("write", set(strings(sign.get("permissions", {}))))
        self.assertIn("build", needs(sign), "the sign job signs this run's build artifacts")
        self.assertEqual(workflow["env"]["FORK_KEY_FINGERPRINT"], FORK_KEY_FINGERPRINT)
        self.assertTrue(
            any("$FORK_KEY_FINGERPRINT" in script(step) and "--list-secret-keys" in script(step) for step in steps(sign)),
            "the sign job checks the imported key's fingerprint",
        )
        publish = [
            name for name, job in jobs.items()
            if any("action-gh-release" in action(step) or "gh release create" in script(step) for step in steps(job))
        ]
        self.assertEqual(len(publish), 1)
        self.assertIn("sign", needs(jobs[publish[0]]), "publishing waits for the signatures")

    def test_build_signs_only_when_a_caller_asks(self):
        build = load("build.yml")
        spec = build["on"]["workflow_call"]
        sign_input = spec["inputs"]["sign_in_build"]
        self.assertEqual(sign_input["type"], "boolean")
        self.assertIs(sign_input["default"], True, "upstream's callers keep signing in the build")
        for secret in ("GPG_PRIVATE_KEY", "GPG_PASSPHRASE"):
            self.assertIs(spec["secrets"][secret]["required"], False)
        for job_name, job in build["jobs"].items():
            for step in steps(job):
                if mentions(step, KEY_NAMES) or "import-gpg" in action(step) or SIGNING_COMMAND.search(script(step)):
                    with self.subTest(job=job_name, step=step.get("name")):
                        self.assertEqual(step.get("if"), "inputs.sign_in_build")
        for caller in ("release.yml", "nightly.yml"):
            for job in build_calls(load(caller)).values():
                self.assertNotIn("sign_in_build", job.get("with", {}), caller)


class GuardTests(unittest.TestCase):
    def assert_guarded(self, name, guard):
        for job_name, job in load(name)["jobs"].items():
            condition = str(job.get("if", ""))
            with self.subTest(workflow=name, job=job_name):
                if not needs(job):
                    self.assertEqual(condition.strip(), guard, "a job without needs carries the repository guard")
                elif re.search(r"\b(?:always|failure|cancelled)\(\)", condition):
                    # Such a job runs after a skipped dependency, so it needs its own guard.
                    self.assertIn(guard, condition)

    def test_upstream_only_workflows_never_run_in_the_fork(self):
        for name in UPSTREAM_ONLY:
            self.assert_guarded(name, UPSTREAM_GUARD)

    def test_fork_workflows_never_run_upstream(self):
        for name in FORK:
            self.assert_guarded(name, FORK_GUARD)


class SupplyChainTests(unittest.TestCase):
    def test_every_action_is_pinned_to_a_full_commit_sha(self):
        for path in sorted(WORKFLOWS.glob("*.y*ml")):
            for job_name, job in load(path.name)["jobs"].items():
                references = [job["uses"]] if "uses" in job else []
                references += [step["uses"] for step in steps(job) if "uses" in step]
                for reference in references:
                    if reference.startswith("./"):
                        continue
                    with self.subTest(workflow=path.name, job=job_name, uses=reference):
                        if reference.startswith("docker://"):
                            self.assertRegex(reference, r"@sha256:[0-9a-f]{64}$")
                        else:
                            self.assertRegex(reference, FULL_SHA)

    def test_fork_builds_wait_for_the_lock_gate(self):
        for name in FORK:
            with self.subTest(workflow=name):
                jobs = load(name)["jobs"]
                gates = {job_name for job_name, job in jobs.items() if any(LOCK_GATE.search(script(step)) for step in steps(job))}
                self.assertTrue(gates, "no job checks Cargo.lock with --locked")
                for job in build_calls({"jobs": jobs}).values():
                    self.assertTrue(gates & set(needs(job)), "the build does not wait for the lock gate")

    def test_no_untrusted_expressions_in_run_scripts(self):
        for name in FORK_REACHABLE:
            for job_name, job in load(name)["jobs"].items():
                for step in steps(job):
                    run = str(step.get("run", ""))
                    with self.subTest(workflow=name, job=job_name, step=step.get("name")):
                        self.assertIsNone(UNTRUSTED_EXPRESSION.search(run), "route the value through env:")

    def test_checkouts_do_not_persist_credentials(self):
        for name in FORK_REACHABLE:
            for job_name, job in load(name)["jobs"].items():
                for step in steps(job):
                    if action(step) == "actions/checkout":
                        with self.subTest(workflow=name, job=job_name):
                            self.assertIs(step.get("with", {}).get("persist-credentials"), False)

    def test_build_tools_and_images_are_pinned(self):
        build = load("build.yml")
        for job_name, job in build["jobs"].items():
            for step in steps(job):
                run = script(step)
                with self.subTest(job=job_name, step=step.get("name")):
                    self.assertNotRegex(run, r"\bbrew\s+install\b", "install tools from a pinned release with a checksum")
                    if action(step) == "actions/setup-go":
                        self.assertRegex(str(step["with"]["go-version"]), r"^\d+\.\d+\.\d+$")
                    for command in re.findall(r"docker run[^\n]*", run):
                        self.assertTrue(
                            '"$BUILD_CONTAINER"' in command or re.search(r"@sha256:[0-9a-f]{64}", command),
                            f"unpinned image in: {command}",
                        )
        container = build["jobs"]["build-linux"]["env"]["BUILD_CONTAINER"]
        self.assertRegex(container, r"^[\w./-]+:[\w.-]+@sha256:[0-9a-f]{64}$")

    def test_release_builds_restore_no_cache(self):
        build = load("build.yml")
        for job_name, job in build["jobs"].items():
            for step in steps(job):
                if action(step) == "Swatinem/rust-cache":
                    with self.subTest(job=job_name):
                        self.assertTrue(step.get("if"), "every cache restore is gated by an input")
        windows = [step for step in steps(build["jobs"]["build-windows"]) if action(step) == "Swatinem/rust-cache"]
        self.assertEqual([step.get("if") for step in windows], ["inputs.windows_cache"])
        for job in build_calls(load("fork-release.yml")).values():
            self.assertIs(job["with"].get("cache"), False)
            self.assertIs(job["with"].get("windows_cache"), False)

    def test_release_notes_list_only_this_repositorys_pull_requests(self):
        notes = load("fork-release.yml")["jobs"]["notes"]
        listing = [script(step) for step in steps(notes) if "gh pr list" in script(step)]
        self.assertEqual(len(listing), 1)
        self.assertIn("isCrossRepository", listing[0])
        self.assertIn("select(.isCrossRepository == false)", listing[0])


if __name__ == "__main__":
    unittest.main()

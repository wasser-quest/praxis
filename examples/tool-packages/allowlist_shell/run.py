"""execute_terminal replacement: allowlisted argv, no shell, 60 s limit."""
import json, os, shlex, subprocess, sys

ALLOWED = {
    "ls": None, "cat": None, "head": None, "tail": None, "wc": None,
    "grep": None, "find": None,
    "git": {"status", "log", "diff", "show"},
    "cargo": {"check", "test", "build", "fmt"},
}

def main():
    args = json.loads(os.environ.get("PLUGIN_ARGS", "{}"))
    try:
        argv = shlex.split(args.get("command", ""))
    except ValueError as error:
        return f"Error: {error}"
    if not argv:
        return "Error: empty command"
    if any(token in {"|", "&&", "||", ";", ">", ">>", "<"} for token in argv):
        return "Error: pipes, redirects and command chaining are not allowed"
    sub = ALLOWED.get(argv[0], False)
    if sub is False or (sub is not None and (len(argv) < 2 or argv[1] not in sub)):
        return f"Error: '{' '.join(argv[:2])}' is not allowlisted"
    if argv[0] == "find" and any(a in {"-exec", "-execdir", "-delete", "-ok"} for a in argv):
        return "Error: find actions are not allowed"
    try:
        done = subprocess.run(argv, capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired:
        return "Error: timed out after 60 s"
    except FileNotFoundError:
        return f"Error: {argv[0]} is not installed"
    out = (done.stdout + done.stderr)[-20000:]
    return f"exit {done.returncode}\n{out}"

if __name__ == "__main__":
    sys.stdout.write(main())

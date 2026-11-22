import time


MANAGE = "cordis.manage@2.0"


def terminal_outcome(snapshot, caller, ticket):
    if snapshot is None:
        return None
    return next((row for row in snapshot["last_batch"]
                 if row["caller"] == caller and row["id"] == ticket
                 and row["state"] != "pending"), None)


def completed_reply(accepted, outcome):
    succeeded = outcome["state"] in ("succeeded", "committed_with_cleanup")
    return {**accepted, "ok": succeeded, "code": outcome.get("code"),
            "value": outcome.get("error") if not succeeded else None,
            "ticket": accepted["value"], "outcome": outcome}


def request_admin(current, method, arguments, *, expect_success=True):
    started = time.monotonic()
    reply, _ = current.request("admin", MANAGE, method, arguments, expect_success=False)
    if reply["ok"] and method != "install":
        ticket = reply["value"]["id"]
        remaining = max(0, 180 - (time.monotonic() - started))
        outcome = current.wait(lambda: terminal_outcome(current.status(), "admin", ticket),
                               f"{method} 票据 {ticket} 达到终态", seconds=remaining)
        reply = completed_reply(reply, outcome)
    if expect_success:
        assert reply["ok"], reply
    return reply

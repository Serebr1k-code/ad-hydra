"""Hydra's built-in ForcAD submitter. Config is a private, short-lived file."""
import http.client
import ipaddress
import json
import sys
from pathlib import Path

MAX_RESPONSE = 65536
MARKER = "HYDRA_FORCAD_RESULTS="


def main():
    with Path(__file__).with_name("forcad.json").open("r", encoding="utf-8") as config_file:
        config = json.load(config_file)
    address = ipaddress.ip_address(config["ip"])
    port = int(config["port"])
    token = config["team_token"]
    flags = sys.argv[1:]
    if not flags or len(flags) > 100 or any(not f or "\0" in f for f in flags):
        raise ValueError("invalid ForcAD flag batch")

    # HTTPConnection connects directly to a literal address: environment proxies,
    # DNS, URL credentials and redirects cannot redirect the team token elsewhere.
    host = address.compressed
    connection = http.client.HTTPConnection(host, port, timeout=15)
    body = json.dumps(flags, ensure_ascii=False, separators=(",", ":")).encode("utf-8")
    try:
        connection.request("PUT", "/flags/", body=body, headers={
            "Content-Type": "application/json",
            "X-Team-Token": token,
            "Connection": "close",
        })
        response = connection.getresponse()
        payload = response.read(MAX_RESPONSE + 1)
        if len(payload) > MAX_RESPONSE or response.status in (408, 425, 429) or response.status >= 500:
            raise RuntimeError("ForcAD returned a retryable HTTP response")
        if response.status != 200:
            if response.status == 400:
                try:
                    error = json.loads(payload.decode("utf-8")).get("error", "")
                except Exception:
                    error = ""
                if isinstance(error, str) and "invalid team token" in error.lower():
                    results = ["credential"] * len(flags)
                    print(MARKER + json.dumps(results, separators=(",", ":")))
                    return 0
            raise RuntimeError(f"ForcAD returned HTTP {response.status}")
        data = json.loads(payload.decode("utf-8"))
        if not isinstance(data, list) or len(data) != len(flags):
            raise RuntimeError("ForcAD response shape did not match the submitted batch")
        results = []
        for expected, item in zip(flags, data):
            if not isinstance(item, dict) or item.get("flag") != expected or not isinstance(item.get("msg"), str):
                raise RuntimeError("ForcAD response flag did not match the submitted batch")
            prefix = f"[{expected}] "
            message = item["msg"]
            if not message.startswith(prefix):
                raise RuntimeError("ForcAD response message did not match its flag")
            verdict = message[len(prefix):].strip().lower()
            if verdict.startswith("flag accepted!"):
                results.append("accepted")
            elif verdict.startswith("flag already stolen"):
                results.append("already_stolen")
            elif verdict.startswith("flag is your own"):
                results.append("own")
            elif verdict.startswith("flag is invalid") or verdict.startswith("flag is too old"):
                results.append("invalid")
            elif verdict.startswith("cannot submit flags while service is down") or verdict.startswith("game is not available"):
                results.append("retry")
            else:
                results.append("unknown")
        print(MARKER + json.dumps(results, separators=(",", ":")))
    finally:
        connection.close()


if __name__ == "__main__":
    try:
        main()
    except Exception:
        print("ForcAD submission failed; the batch will be retried.", file=sys.stderr)
        sys.exit(1)

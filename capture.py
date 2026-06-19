"""mitmdump addon: log every Slack Web API request (headers + body) so we can
see exactly how the desktop app authenticates (token param, Cookie header,
extra _x_* params)."""
from mitmproxy import http

LOG = r"C:\Users\ryuji\tagamiAi\slack_capture.txt"


def request(flow: http.HTTPFlow) -> None:
    url = flow.request.pretty_url
    if "slack.com/api/" not in url:
        return
    lines = []
    lines.append("==== %s %s ====" % (flow.request.method, url))
    for k, v in flow.request.headers.items():
        lines.append("H %s: %s" % (k, v))
    try:
        body = flow.request.get_text() or ""
        lines.append("BODY: %s" % body[:3000])
    except Exception as e:  # noqa
        lines.append("BODY-ERR %s" % e)
    lines.append("")
    with open(LOG, "a", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")

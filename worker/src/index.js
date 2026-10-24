// ytdr-report: riceve le segnalazioni inviate da YTDownloaderRUST e apre una
// issue nel repository GitHub. La chiave GitHub sta solo qui, come segreto
// del Worker (GITHUB_TOKEN), mai nell'app.

const REPO = "gmy77/YTDownloaderRUST";
const MAX_TITLE = 120;
const MAX_BODY = 12000;
const PER_IP_PER_HOUR = 5;
const PER_DAY = 50;

const json = (data, status = 200) =>
  new Response(JSON.stringify(data), { status, headers: { "content-type": "application/json; charset=utf-8" } });

async function sha256(text) {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(text));
  return [...new Uint8Array(digest)].map((b) => b.toString(16).padStart(2, "0")).join("");
}

// "@nome" su GitHub notifica quell'utente: lo si disinnesca con uno spazio invisibile.
const defuse = (s) => s.replace(/@(?=[A-Za-z0-9-])/g, "@​");

// Conta in KV con scadenza; restituisce false se il limite è superato.
async function allow(env, key, limit, ttl) {
  const n = Number((await env.RL.get(key)) || 0);
  if (n >= limit) return false;
  await env.RL.put(key, String(n + 1), { expirationTtl: ttl });
  return true;
}

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (request.method === "GET" && url.pathname === "/") {
      return json({ service: "ytdr-report", configured: Boolean(env.GITHUB_TOKEN) });
    }
    if (request.method !== "POST" || url.pathname !== "/report") return json({ error: "not_found" }, 404);
    if (!(request.headers.get("User-Agent") || "").startsWith("YTDownloaderRUST/")) return json({ error: "client" }, 403);
    if (!env.GITHUB_TOKEN) return json({ error: "not_configured" }, 503);
    if (Number(request.headers.get("Content-Length") || 0) > 4 * MAX_BODY) return json({ error: "too_large" }, 413);

    let data;
    try {
      data = await request.json();
    } catch {
      return json({ error: "bad_json" }, 400);
    }
    const title = typeof data.title === "string" ? data.title.trim().slice(0, MAX_TITLE) : "";
    const body = typeof data.body === "string" ? data.body.trim().slice(0, MAX_BODY) : "";
    const version = typeof data.version === "string" ? data.version.replace(/[^0-9A-Za-z.\-]/g, "").slice(0, 20) : "?";
    if (!title || !body) return json({ error: "empty" }, 400);

    // Limiti: per IP (solo impronta, l'IP non viene salvato) e totale giornaliero.
    const now = Date.now();
    const ip = await sha256(request.headers.get("CF-Connecting-IP") || "unknown");
    if (!(await allow(env, `ip:${ip}:${Math.floor(now / 3600000)}`, PER_IP_PER_HOUR, 3700))) {
      return json({ error: "rate_limited" }, 429);
    }
    if (!(await allow(env, `day:${Math.floor(now / 86400000)}`, PER_DAY, 90000))) {
      return json({ error: "daily_limit" }, 429);
    }

    const gh = await fetch(`https://api.github.com/repos/${REPO}/issues`, {
      method: "POST",
      headers: {
        Authorization: `Bearer ${env.GITHUB_TOKEN}`,
        Accept: "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "ytdr-report",
      },
      body: JSON.stringify({
        title: `[v${version}] ${defuse(title)}`,
        body: `${defuse(body)}\n\n---\n_Inviata dall'app YTDownloaderRUST v${version} tramite ytdr-report._`,
        labels: ["segnalazione"],
      }),
    });
    if (!gh.ok) return json({ error: "github", status: gh.status }, 502);
    const issue = await gh.json();
    return json({ number: issue.number }, 201);
  },
};

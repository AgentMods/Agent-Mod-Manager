import { publicHttpsUrl } from "./public-url";
export { publicHttpsUrl } from "./public-url";

// Always use the manager's default GitHub mirror, including in the browser preview.
const sponsorManifest = "https://cdn.gh-proxy.org/https://raw.githubusercontent.com/AgentMods/Agent-Mod-Manager/refs/heads/main/Sponsors.json";

export type Sponsor = {
  name: string;
  logo: string | null;
  url: string | null;
};

export function sponsorHost(url: string | null): string | null {
  if (!url) return null;
  try {
    return new URL(url).hostname.replace(/\.$/, "").replace(/^www\./i, "") || null;
  } catch {
    return null;
  }
}

export async function loadSponsors(): Promise<Sponsor[]> {
  const response = await fetch(sponsorManifest, {
    credentials: "omit", referrerPolicy: "no-referrer", redirect: "error", signal: AbortSignal.timeout(8000),
  });
  if (!response.ok) throw new Error(`Sponsor manifest: HTTP ${response.status}`);
  const entries: unknown = await response.json();
  if (!Array.isArray(entries)) throw new Error("Invalid sponsor manifest");
  const seen = new Set<string>();
  const sponsors: Sponsor[] = [];
  for (const entry of entries) {
    if (!entry || typeof entry !== "object" || typeof (entry as { name?: unknown }).name !== "string") continue;
    const record = entry as { name: string; logo?: unknown; url?: unknown };
    const name = record.name.trim();
    if (!name || seen.has(name)) continue;
    seen.add(name);
    sponsors.push({
      name,
      logo: publicHttpsUrl(record.logo),
      url: publicHttpsUrl(record.url),
    });
  }
  return sponsors;
}

export async function loadSponsorNames(): Promise<string[]> {
  return (await loadSponsors()).map(sponsor => sponsor.name);
}

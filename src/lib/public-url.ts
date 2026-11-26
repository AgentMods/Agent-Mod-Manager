export function publicHttpsUrl(value: unknown): string | null {
  if (typeof value !== "string") return null;
  const trimmed = value.trim();
  if (!trimmed.startsWith("https://") || trimmed.length > 2048 || /[\s\\\p{Cc}]/u.test(trimmed)) return null;
  try {
    const parsed = new URL(trimmed);
    const host = parsed.hostname.replace(/\.$/, "");
    if (
      parsed.protocol !== "https:" ||
      !host.includes(".") ||
      /^[\d.]+$/.test(host) ||
      /\.(?:localhost|local|internal)$/i.test(host) ||
      parsed.username ||
      parsed.password ||
      parsed.port
    ) {
      return null;
    }
    return trimmed;
  } catch {
    return null;
  }
}

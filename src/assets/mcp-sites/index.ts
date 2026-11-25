// Bundle only files that exist; missing artwork keeps the generic server icon.
const icons = import.meta.glob<string>("./*.png", { eager: true, import: "default" });

// Keys are built-in MCP preset IDs; display names can be edited by the user.
export function mcpSiteLogo(id: string): string | undefined {
  const filename = id === "glossmod_3dm" ? "3dm" : id === "glossmod_http" ? "glossmod" : id;
  return icons[`./${filename}.png`];
}

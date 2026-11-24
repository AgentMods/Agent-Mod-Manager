import { getLocale, type Locale } from "./i18n";
import catalog from "../../src-tauri/locales/native.json";
import detailCatalog from "../../src-tauri/locales/error-details.json";

export type NativeMessage = {
  key: string;
  args: Readonly<Record<string, string | number>>;
  message: string;
};

type Translations = Readonly<Partial<Record<Locale, string>>>;
type ErrorDetails = Readonly<Record<string, Readonly<Record<string, Translations>>>>;
const messages: Readonly<Record<string, Translations>> = catalog;
const details: ErrorDetails = detailCatalog;

function translations(key: string) {
  return Object.hasOwn(messages, key) ? messages[key] : undefined;
}

function translateDetail(key: string, source: string, locale: Locale): string {
  if (locale === "zh-CN" || !Object.hasOwn(details, key)) return source;
  const category = details[key];
  // App-authored literals are keyed by the entire source, not by a substring
  // or guessed pattern. Uncatalogued paths and diagnostics pass through as-is.
  const entry = Object.hasOwn(category, source) ? category[source] : undefined;
  return entry?.[locale] ?? entry?.en ?? source;
}

function render(value: { key: string; message: string; args?: unknown }): string {
  const locale = getLocale();
  if (value.key === "error.raw") return translateDetail(value.key, value.message, locale);
  const entries = translations(value.key);
  const template = entries?.[locale] ?? entries?.en;
  if (template === undefined) return value.message;
  const args = value.args;
  let missing = false;
  const translated = template.replace(/\{([a-zA-Z_]\w*)\}/g, (token, name: string) => {
    if (args && typeof args === "object" && Object.hasOwn(args, name)) {
      const argument = (args as Record<string, unknown>)[name];
      if (typeof argument === "string") {
        return name === "detail" ? translateDetail(value.key, argument, locale) : argument;
      }
      if (typeof argument === "number") return String(argument);
    }
    missing = true;
    return token;
  });
  return missing ? value.message : translated;
}

function structured(value: unknown): value is { key: string; message: string; args?: unknown } {
  return value !== null && typeof value === "object" && "key" in value &&
    typeof value.key === "string" && "message" in value && typeof value.message === "string";
}

/** Render native errors (or scan warnings) without treating raw diagnostic text as a template. */
export function formatNativeError(value: unknown): string {
  if (structured(value)) return render(value);
  if (value instanceof Error) {
    return structured(value.cause) ? render(value.cause) : value.message;
  }
  if (typeof value === "string") return value;
  if (value && typeof value === "object" && "message" in value && typeof value.message === "string") {
    return value.message;
  }
  const unknown = render({ key: "error.unknown", message: "未知错误" });
  if (value == null) return unknown;
  try { return JSON.stringify(value) ?? unknown; } catch { return unknown; }
}

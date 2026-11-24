import { useSyncExternalStore } from "react";
import { pluralTemplate } from "../locales/plurals";

export const languages = {
  "zh-CN": "简体中文", en: "English", ja: "日本語", ko: "한국어", de: "Deutsch",
  fr: "Français", es: "Español", "pt-BR": "Português (Brasil)", ru: "Русский", it: "Italiano",
} as const;
export type Locale = keyof typeof languages;
export type LanguageSettings = { locale: Locale | null; effective_locale: Locale };
export function isLocale(value: unknown): value is Locale {
  return typeof value === "string" && Object.hasOwn(languages, value);
}

export function matchLocales(preferences: readonly string[]): Locale {
  for (const preference of preferences) {
    try {
      const tag = preference.trim().split(/[.@]/, 1)[0].replaceAll("_", "-");
      const language = new Intl.Locale(tag).language;
      if (language === "zh") return "zh-CN";
      if (language === "pt") return "pt-BR";
      if (isLocale(language)) return language;
    } catch { /* Try the next language in the system's preference list. */ }
  }
  return "en";
}

const previewPreference = "amm.language";
export function browserLanguageSettings(): LanguageSettings {
  let selected: string | null = null;
  try { selected = localStorage.getItem(previewPreference); } catch { /* Restricted WebView. */ }
  const locale = isLocale(selected) ? selected : null;
  return { locale, effective_locale: locale ?? matchLocales(navigator.languages.length ? navigator.languages : [navigator.language]) };
}
export function saveBrowserLanguage(locale: Locale | null) {
  if (locale) localStorage.setItem(previewPreference, locale);
  else localStorage.removeItem(previewPreference);
  return browserLanguageSettings();
}

// Chinese source messages are the source catalog. Only app-authored copy is passed to tr().
const catalogs = import.meta.glob<Record<string, string>>("../locales/translations/*.json", { eager: true, import: "default" });
let settings = browserLanguageSettings();
const listeners = new Set<() => void>();
const subscribe = (listener: () => void) => { listeners.add(listener); return () => { listeners.delete(listener); }; };
export const getLocale = () => settings.effective_locale;
export const getLanguageSettings = () => settings;
export const useLanguageSettings = () => useSyncExternalStore(subscribe, getLanguageSettings);
export const useLocale = () => useSyncExternalStore(subscribe, getLocale);
export function applyLanguageSettings(next: LanguageSettings) {
  if (!isLocale(next.effective_locale) || (next.locale !== null && !isLocale(next.locale))) return;
  document.documentElement.lang = next.effective_locale;
  if (settings.locale === next.locale && getLocale() === next.effective_locale) return;
  settings = next;
  for (const listener of listeners) listener();
}

export function formatList(values: string[]) {
  const locale = getLocale();
  if (locale === "zh-CN") return values.join("、");
  return new Intl.ListFormat(locale, { style: "short", type: "conjunction" }).format(values);
}

type Values = Record<string, string | number | boolean | null | undefined>;
export function tr(source: string, values: Values = {}): string {
  const locale = getLocale();
  const catalog = catalogs[`../locales/translations/${locale}.json`];
  const translated = catalog && Object.hasOwn(catalog, source) ? catalog[source] : source;
  const template = pluralTemplate(locale, source, values) ?? translated;
  return template.replace(/\{(\w+)\}/g, (token, name: string) => {
    if (!Object.hasOwn(values, name)) return token;
    const value = values[name];
    if (typeof value === "number") return value.toLocaleString(locale);
    return value == null || typeof value === "boolean" ? "" : String(value);
  });
}

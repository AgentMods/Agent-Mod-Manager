import { useEffect, useState } from "react";
import { backendAvailable, api, errorText } from "@/lib/api";
import { applyLanguageSettings, browserLanguageSettings, getLanguageSettings, saveBrowserLanguage, type LanguageSettings, type Locale } from "./i18n";

// A focus refresh may finish after a user choice. Apply only the newest request.
let generation = 0;
let saving = false;
async function refreshLanguage() {
  if (saving) return;
  const request = ++generation;
  const next = backendAvailable ? await api<LanguageSettings>("get_app_locale") : browserLanguageSettings();
  if (request === generation) applyLanguageSettings(next);
}
export async function initializeLanguage() {
  applyLanguageSettings(getLanguageSettings());
  // The first visible label should already respect a saved desktop override.
  try { await refreshLanguage(); } catch { /* Browser preferences are available offline. */ }
}
export function useLanguageSync() {
  useEffect(() => {
    const refresh = () => { void refreshLanguage().catch(() => { /* Startup and focus can precede native service availability. */ }); };
    window.addEventListener("focus", refresh);
    window.addEventListener("languagechange", refresh);
    return () => { window.removeEventListener("focus", refresh); window.removeEventListener("languagechange", refresh); };
  }, []);
}
export function useSaveLanguage() {
  const [pending, setPending] = useState(false);
  const [error, setError] = useState("");
  async function save(locale: Locale | null) {
    if (saving) return;
    saving = true; ++generation;
    setPending(true); setError("");
    try {
      const next = backendAvailable ? await api<LanguageSettings>("set_app_locale", { locale }) : saveBrowserLanguage(locale);
      applyLanguageSettings(next);
    } catch (cause) { setError(errorText(cause)); }
    finally { saving = false; setPending(false); }
  }
  return { save, pending, error };
}

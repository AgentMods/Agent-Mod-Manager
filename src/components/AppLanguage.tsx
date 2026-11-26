import { LanguagesIcon } from "lucide-react";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { isLocale, languages, tr, useLanguageSettings } from "@/lib/i18n";
import { useSaveLanguage } from "@/lib/app-language";

export function AppLanguage() {
  const { locale, effective_locale } = useLanguageSettings();
  const { save, pending, error } = useSaveLanguage();
  return <Field className="justify-between gap-1.5 rounded-xl border bg-muted/25 px-3.5 py-2.5 transition-colors duration-200 hover:border-foreground/20" data-invalid={!!error}>
    <div className="flex flex-wrap items-center justify-between gap-2 [&_[data-slot=native-select-wrapper]]:w-full sm:[&_[data-slot=native-select-wrapper]]:w-fit sm:[&_[data-slot=native-select-wrapper]]:max-w-full">
      <FieldLabel htmlFor="app-locale" className="flex shrink-0 items-center gap-2">
        <LanguagesIcon aria-hidden="true" className="size-4 text-muted-foreground" />
        {tr("语言")}
      </FieldLabel>
      <NativeSelect id="app-locale" size="sm" aria-describedby="app-locale-help" aria-invalid={!!error} value={locale ?? ""} disabled={pending}
        onChange={event => { const value = event.target.value; if (value === "") void save(null); else if (isLocale(value)) void save(value); }}>
        <NativeSelectOption value="">{locale ? tr("跟随系统") : tr("跟随系统（{language}）", { language: languages[effective_locale] })}</NativeSelectOption>
        {Object.entries(languages).map(([code, name]) => <NativeSelectOption key={code} value={code} lang={code}>{name}</NativeSelectOption>)}
      </NativeSelect>
    </div>
    <FieldDescription id="app-locale-help" className="text-xs leading-snug">{tr("用于界面和今后安装的 Mod 名称与标签。")}</FieldDescription>
    {error && <FieldError className="text-xs">{error}</FieldError>}
  </Field>;
}

import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { isLocale, languages, tr, useLanguageSettings } from "@/lib/i18n";
import { useSaveLanguage } from "@/lib/app-language";

export function AppLanguage() {
  const { locale, effective_locale } = useLanguageSettings();
  const { save, pending, error } = useSaveLanguage();
  return <Field className="mt-5" data-invalid={!!error}>
    <FieldLabel htmlFor="app-locale">{tr("语言")}</FieldLabel>
    <NativeSelect id="app-locale" aria-describedby="app-locale-help" aria-invalid={!!error} value={locale ?? ""} disabled={pending}
      onChange={event => { const value = event.target.value; if (value === "") void save(null); else if (isLocale(value)) void save(value); }}>
      <NativeSelectOption value="">{locale ? tr("跟随系统") : tr("跟随系统（{language}）", { language: languages[effective_locale] })}</NativeSelectOption>
      {Object.entries(languages).map(([code, name]) => <NativeSelectOption key={code} value={code} lang={code}>{name}</NativeSelectOption>)}
    </NativeSelect>
    <FieldDescription id="app-locale-help">{tr("用于界面和今后安装的 Mod 名称与标签。")}</FieldDescription>
    {error && <FieldError>{error}</FieldError>}
  </Field>;
}

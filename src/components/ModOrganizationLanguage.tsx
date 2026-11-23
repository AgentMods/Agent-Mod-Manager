import { useMutation } from "@tanstack/react-query";
import { api, backendAvailable, useBackendQuery } from "@/lib/api";
import { Field, FieldDescription, FieldError, FieldLabel } from "@/components/ui/field";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";

type LanguageSettings = {
  locale: string | null;
  effective_locale: string;
  languages: [code: string, label: string][];
};

export function ModOrganizationLanguage() {
  const settings = useBackendQuery<LanguageSettings>(["mod-organization-locale"], "get_mod_organization_locale");
  const save = useMutation({
    mutationFn: (locale: string | null) => api<void>("set_mod_organization_locale", { locale }),
    onSuccess: async () => { await settings.refetch(); },
  });
  const data = settings.data;
  const error = save.error ?? settings.error;

  return <Field className="mt-5" data-invalid={!!error}>
    <FieldLabel htmlFor="mod-organization-locale">Mod 整理目标语言</FieldLabel>
    <NativeSelect
      id="mod-organization-locale"
      aria-describedby="mod-organization-locale-description"
      aria-invalid={!!error}
      disabled={!data || save.isPending}
      value={data?.locale ?? ""}
      onChange={(event) => save.mutate(event.target.value || null)}
    >
      <NativeSelectOption value="">跟随系统{data && !data.locale ? `（${data.languages.find(([code]) => code === data.effective_locale)?.[1] ?? data.effective_locale}）` : ""}</NativeSelectOption>
      {data?.languages.map(([code, label]) => <NativeSelectOption key={code} value={code}>{label}</NativeSelectOption>)}
    </NativeSelect>
    <FieldDescription id="mod-organization-locale-description">
      {!backendAvailable ? "请在桌面应用中更改。" : "仅用于今后安装后的 Mod 本地化，不更改界面语言或已安装的 Mod。"}
    </FieldDescription>
    {error && <FieldError>{error.message}</FieldError>}
  </Field>;
}

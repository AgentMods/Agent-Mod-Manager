import { useId } from 'react';
import { CircleCheckIcon, TriangleAlertIcon } from 'lucide-react';
import { Alert, AlertDescription, AlertTitle } from '@/components/ui/alert';
import { Checkbox } from '@/components/ui/checkbox';
import { Field, FieldContent, FieldDescription, FieldGroup, FieldLabel, FieldLegend, FieldSet } from '@/components/ui/field';
import { RadioGroup, RadioGroupItem } from '@/components/ui/radio-group';
import { Table, TableBody, TableCell, TableHead, TableHeader, TableRow } from '@/components/ui/table';
import { selectedFomodMappings, validFomodSelection, type FomodPreview, type FomodSelections } from '@/lib/fomod';

export function FomodOptions({ preview, selections, onChange, disabled }: {
  preview: FomodPreview;
  selections: FomodSelections;
  onChange: (next: FomodSelections) => void;
  disabled: boolean;
}) {
  const id = useId();
  const { automaticSelection, groups } = preview;
  const safe = automaticSelection.status === 'safe';
  const StatusIcon = safe ? CircleCheckIcon : TriangleAlertIcon;
  const mappings = selectedFomodMappings(preview, selections);
  const warnings = new Set([...automaticSelection.warnings, ...preview.warnings]);

  return <section className="flex min-w-0 flex-col gap-4" aria-labelledby={id}>
    <div className="flex flex-col gap-1 [overflow-wrap:anywhere]">
      <h3 id={id} className="font-semibold">FOMOD 安装选项</h3>
      <small className="text-muted-foreground">{preview.moduleName ?? preview.configPath}</small>
    </div>
    <Alert variant={safe ? 'success' : 'warning'} role="status">
      <StatusIcon aria-hidden="true"/>
      <AlertTitle className="line-clamp-none">{safe ? (groups.length ? '已预选确定性选项' : '仅包含必选文件') : '需要用户选择'}</AlertTitle>
      <AlertDescription>{safe ? '请核对下方映射，再生成变更预览。' : '请选择安装项，并根据模组说明核对文件映射。'}</AlertDescription>
    </Alert>
    {groups.map((group, groupIndex) => {
      const selected = selections[groupIndex] ?? [];
      const radio = group.selectionType === 'SelectExactlyOne' || group.selectionType === 'SelectOne';
      const single = group.selectionType === 'SelectAtMostOne';
      const legendId = `${id}-${groupIndex}`;
      const options = group.options.map((option, optionIndex) => {
        const optionId = `${legendId}-${optionIndex}`;
        const nameId = `${optionId}-name`;
        const control = radio
          ? <RadioGroupItem id={optionId} value={String(optionIndex)} aria-labelledby={nameId}/>
          : <Checkbox id={optionId} aria-labelledby={nameId} disabled={disabled} checked={selected.includes(optionIndex)}
              onCheckedChange={checked => {
                const next = checked === true
                  ? (single ? [optionIndex] : [...selected, optionIndex])
                  : selected.filter(index => index !== optionIndex);
                onChange({ ...selections, [groupIndex]: next });
              }}/>;

        return <FieldLabel htmlFor={optionId} key={optionIndex} className="min-w-0">
          <Field orientation="horizontal">
            {control}
            <FieldContent className="min-w-0 [overflow-wrap:anywhere]">
              <strong id={nameId}>{option.name}</strong>
              {option.description && <FieldDescription>{option.description}</FieldDescription>}
              {option.hasConditions && <small className="text-warning">含未判定的条件或规则</small>}
            </FieldContent>
          </Field>
        </FieldLabel>;
      });

      return <FieldSet className="min-w-0" disabled={disabled} key={groupIndex}>
        <FieldLegend id={legendId} className="max-w-full [overflow-wrap:anywhere]">
          {group.name} <small className="text-muted-foreground">{group.selectionType}</small>
          {group.hasConditions && <small className="ml-2 text-warning">含未判定的条件或规则</small>}
        </FieldLegend>
        {radio ? <RadioGroup aria-labelledby={legendId} disabled={disabled}
          value={selected.length ? String(selected[0]) : ''}
          onValueChange={value => onChange({ ...selections, [groupIndex]: [Number(value)] })}>
          {options}
        </RadioGroup> : <FieldGroup className="gap-3">{options}</FieldGroup>}
      </FieldSet>;
    })}
    <div className="flex min-w-0 flex-col gap-2">
      <h4 className="font-medium">文件映射预览</h4>
      {mappings.length ? <div className="max-h-64 min-w-0 overflow-auto rounded-md border">
        <Table aria-label="文件映射预览" className="table-fixed">
          <TableHeader><TableRow><TableHead>归档源文件</TableHead><TableHead>安装目标</TableHead></TableRow></TableHeader>
          <TableBody>{mappings.map((mapping, index) => <TableRow key={index}>
            <TableCell className="whitespace-normal break-all">{mapping.source}</TableCell>
            <TableCell className="whitespace-normal break-all">{mapping.destination}</TableCell>
          </TableRow>)}</TableBody>
        </Table>
      </div> : <p className="text-sm text-muted-foreground">当前选择没有文件映射。</p>}
    </div>
    {[...warnings].map(warning => <p className="text-sm text-warning [overflow-wrap:anywhere]" key={warning}>{warning}</p>)}
    {!validFomodSelection(preview, selections) && <p className="text-sm text-warning" role="status">请按各组的约束完成选择，再生成变更预览。</p>}
  </section>;
}

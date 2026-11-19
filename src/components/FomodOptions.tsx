import { useId } from 'react';
import { CheckCircle2, TriangleAlert } from 'lucide-react';
import { selectedFomodMappings, validFomodSelection, type FomodPreview, type FomodSelections } from '../lib/fomod';

export function FomodOptions({ preview, selections, onChange, disabled }: {
  preview: FomodPreview;
  selections: FomodSelections;
  onChange: (next: FomodSelections) => void;
  disabled: boolean;
}) {
  const id = useId();
  const { automaticSelection, groups } = preview;
  const safe = automaticSelection.status === 'safe';
  const StatusIcon = safe ? CheckCircle2 : TriangleAlert;
  const mappings = selectedFomodMappings(preview, selections);
  const warnings = new Set([...automaticSelection.warnings, ...preview.warnings]);

  return <section className="fomod-preview" aria-labelledby={id}>
    <h3 id={id}>FOMOD 安装选项</h3>
    <small className="muted">{preview.moduleName ?? preview.configPath}</small>
    <div className={`fomod-automatic ${automaticSelection.status}`} role="status">
      <StatusIcon size={18} aria-hidden="true"/>
      <div>
        <strong>{safe ? (groups.length ? '已预选确定性选项' : '仅包含必选文件') : '需要用户选择'}</strong>
        <p>{safe ? '请核对下方映射，再生成变更预览。' : '请选择安装项，并根据模组说明核对文件映射。'}</p>
      </div>
    </div>
    {groups.map((group, groupIndex) => {
      const selected = selections[groupIndex] ?? [];
      const radio = group.selectionType === 'SelectExactlyOne' || group.selectionType === 'SelectOne';
      const single = radio || group.selectionType === 'SelectAtMostOne';
      return <fieldset className="fomod-group" disabled={disabled} key={groupIndex}>
        <legend>
          {group.name} <small className="muted">{group.selectionType}</small>
          {group.hasConditions && <small className="warning-text">含未判定的条件或规则</small>}
        </legend>
        <div className="fomod-options">
          {group.options.map((option, optionIndex) => <label className="fomod-option" key={optionIndex}>
            <input type={radio ? 'radio' : 'checkbox'} name={`${id}-${groupIndex}`}
              checked={selected.includes(optionIndex)} onChange={event => {
                const next = event.currentTarget.checked
                  ? (single ? [optionIndex] : [...selected, optionIndex])
                  : selected.filter(index => index !== optionIndex);
                onChange({ ...selections, [groupIndex]: next });
              }}/>
            <span><strong>{option.name}</strong>
              {option.description && <small>{option.description}</small>}
              {option.hasConditions && <small className="warning-text">含未判定的条件或规则</small>}
            </span>
          </label>)}
        </div>
      </fieldset>;
    })}
    <div className="fomod-block">
      <h4>文件映射预览</h4>
      {mappings.length ? <div className="table-wrap fomod-mappings">
        <table aria-label="文件映射预览">
          <thead><tr><th>归档源文件</th><th>安装目标</th></tr></thead>
          <tbody>{mappings.map((mapping, index) => <tr key={index}>
            <td className="file-path">{mapping.source}</td><td className="file-path">{mapping.destination}</td>
          </tr>)}</tbody>
        </table>
      </div> : <p className="muted">当前选择没有文件映射。</p>}
    </div>
    {[...warnings].map(warning => <p className="warning-text" key={warning}>{warning}</p>)}
    {!validFomodSelection(preview, selections) && <p className="warning-text" role="status">请按各组的约束完成选择，再生成变更预览。</p>}
  </section>;
}

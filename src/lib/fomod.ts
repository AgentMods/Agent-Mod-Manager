type FomodMapping = { source: string; destination: string; priority: number | null };
type FomodOption = {
  name: string;
  description: string | null;
  image: string | null;
  mappings: FomodMapping[];
  hasConditions: boolean;
};
type FomodGroup = { name: string; selectionType: string; options: FomodOption[]; hasConditions: boolean };

export type FomodPreview = {
  artifactPath: string;
  artifactSha256: string;
  configPath: string;
  moduleName: string | null;
  requiredMappings: FomodMapping[];
  groups: FomodGroup[];
  automaticSelection: {
    status: 'safe' | 'requires_user';
    selectedOptions: { group: string; groupIndex: number; option: string; optionIndex: number }[];
    mappings: FomodMapping[];
    warnings: string[];
  };
  warnings: string[];
};

export type FomodSelections = Record<number, number[]>;

export function initialFomodSelections(preview: FomodPreview): FomodSelections {
  const selections: FomodSelections = {};
  if (preview.automaticSelection.status === 'safe') {
    for (const { groupIndex, optionIndex } of preview.automaticSelection.selectedOptions) {
      (selections[groupIndex] ??= []).push(optionIndex);
    }
  }
  return selections;
}

export function selectedFomodMappings(preview: FomodPreview | null, selections: FomodSelections) {
  if (!preview) return [];
  const optionMappings = preview.groups.flatMap((group, groupIndex) =>
    group.options.flatMap((option, optionIndex) => selections[groupIndex]?.includes(optionIndex) ? option.mappings : []),
  );
  return [...preview.requiredMappings, ...optionMappings].map(({ source, destination }) => ({ source, destination }));
}

export function validFomodSelection(preview: FomodPreview, selections: FomodSelections) {
  return preview.groups.every((group, index) => {
    const count = selections[index]?.length ?? 0;
    switch (group.selectionType) {
      case 'SelectExactlyOne':
      case 'SelectOne': return count === 1;
      case 'SelectAtLeastOne': return count > 0;
      case 'SelectAll': return count === group.options.length;
      case 'SelectAtMostOne': return count <= 1;
      default: return true;
    }
  });
}

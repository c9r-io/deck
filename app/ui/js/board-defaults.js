export const DEFAULT_BOARD_SEMANTICS = Object.freeze(['attention', 'working', 'queued', 'parked']);

export function createDefaultColumns(makeId, translate) {
  return DEFAULT_BOARD_SEMANTICS.map(semantic => ({
    id: makeId('C'), semantic, name: translate(`board.default.${semantic}`),
  }));
}

// Exact legacy defaults recover metadata, but displayed names are never
// rewritten. Renamed/user-created Boards therefore remain user data.
export function migrateColumnSemantics(projects) {
  const legacy = { Attention: 'attention', Working: 'working', Queued: 'queued', Parked: 'parked' };
  for (const project of projects || []) for (const column of project.columns || []) {
    if (!column.semantic && legacy[column.name]) column.semantic = legacy[column.name];
  }
  return projects;
}

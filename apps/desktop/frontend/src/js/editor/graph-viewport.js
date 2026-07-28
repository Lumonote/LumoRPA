export const GRAPH_MIN_SCALE = 0.25;
export const GRAPH_MAX_SCALE = 3.5;

export function screenToWorld(transform, screenX, screenY) {
  const scale = transform.scale || 1;
  return {
    x: (screenX - transform.tx) / scale,
    y: (screenY - transform.ty) / scale,
  };
}

export function zoomGraphAt(
  transform,
  screenX,
  screenY,
  factor,
  min = GRAPH_MIN_SCALE,
  max = GRAPH_MAX_SCALE,
) {
  const world = screenToWorld(transform, screenX, screenY);
  const scale = Math.max(min, Math.min(max, transform.scale * factor));
  return {
    tx: screenX - world.x * scale,
    ty: screenY - world.y * scale,
    scale,
  };
}

export function moveNodeByScreenDelta(position, transform, deltaX, deltaY) {
  const scale = transform.scale || 1;
  return {
    x: position.x + deltaX / scale,
    y: position.y + deltaY / scale,
  };
}

export function applyLayoutOverrides(positions, overrides = {}) {
  if (!positions || !overrides) return positions;
  for (const [key, pos] of positions.entries()) {
    const saved = overrides[key];
    if (!saved || !Number.isFinite(saved.x) || !Number.isFinite(saved.y)) continue;
    positions.set(key, { ...pos, x: saved.x, y: saved.y });
  }
  return positions;
}

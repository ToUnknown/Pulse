export function selectionRect(start, end, width, height) {
  const clamp = (value, max) => Math.min(max, Math.max(0, value));
  const x1 = clamp(start.x, width);
  const y1 = clamp(start.y, height);
  const x2 = clamp(end.x, width);
  const y2 = clamp(end.y, height);
  return { x: Math.min(x1, x2), y: Math.min(y1, y2), width: Math.abs(x2 - x1), height: Math.abs(y2 - y1) };
}

// Ratios come from the real captured pixels, not devicePixelRatio (mixed-DPI displays).
export function physicalCrop(rect, viewport, capture) {
  const floor = (value) => Math.floor(value + 1e-7);
  const ceil = (value) => Math.ceil(value - 1e-7);
  const x = Math.max(0, floor(rect.x * capture.width / viewport.width));
  const y = Math.max(0, floor(rect.y * capture.height / viewport.height));
  const right = Math.min(capture.width, ceil((rect.x + rect.width) * capture.width / viewport.width));
  const bottom = Math.min(capture.height, ceil((rect.y + rect.height) * capture.height / viewport.height));
  return { x, y, width: right - x, height: bottom - y };
}

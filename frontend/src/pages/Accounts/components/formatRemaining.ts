/**
 * 把毫秒差格式化成人类可读的短文本。
 *
 * 分档（越长的时长用越粗的单位，避免出现 "284m" 这种要心算的写法）：
 *   < 1 分钟   → `45s`
 *   < 1 小时   → `12m`
 *   < 24 小时  → `4h20m` / `5h`
 *   ≥ 24 小时  → `2天8h` / `3天`
 *
 * 为什么单独一个文件：组件文件只能导出组件，否则 Fast Refresh 会
 * 因为"这个模块既有组件又有普通函数"而整块重挂。
 */
export const formatRemaining = (ms: number): string => {
  const totalSeconds = Math.max(0, Math.round(ms / 1000));

  if (totalSeconds < 60) {
    return `${totalSeconds}s`;
  }

  const totalMinutes = Math.floor(totalSeconds / 60);
  if (totalMinutes < 60) {
    return `${totalMinutes}m`;
  }

  const hours = Math.floor(totalMinutes / 60);
  const minutes = totalMinutes % 60;

  if (hours < 24) {
    return minutes > 0 ? `${hours}h${minutes}m` : `${hours}h`;
  }

  const days = Math.floor(hours / 24);
  const restHours = hours % 24;
  return restHours > 0 ? `${days}天${restHours}h` : `${days}天`;
};

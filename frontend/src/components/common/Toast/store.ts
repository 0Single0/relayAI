/**
 * Toast 的状态与调用入口。
 *
 * 独立成非组件文件：`only-export-components` 要求组件文件只导出组件，
 * 否则改一个常量就会让 Fast Refresh 重挂整棵树。
 */

export type ToastTone = "success" | "error" | "info";

export type ToastItem = {
  id: number;
  tone: ToastTone;
  title: string;
  description?: string;
};

/** 停留时长：失败给更久，因为它通常带原因，需要时间读。 */
const LIFETIME_MS: Record<ToastTone, number> = {
  success: 3000,
  error: 6000,
  info: 4000,
};

let nextID = 1;
let items: ToastItem[] = [];
const listeners = new Set<(items: ToastItem[]) => void>();

const emit = () => {
  for (const listener of listeners) {
    listener([...items]);
  }
};

/** 订阅变化；返回取消订阅函数。 */
export const subscribe = (listener: (items: ToastItem[]) => void) => {
  listeners.add(listener);
  return () => {
    listeners.delete(listener);
  };
};

/** 当前快照（用于初始化 useState）。 */
export const snapshot = () => items;

export const push = (tone: ToastTone, title: string, description?: string) => {
  const id = nextID++;
  items = [...items, { id, tone, title, description }];
  emit();

  window.setTimeout(() => {
    items = items.filter((item) => item.id !== id);
    emit();
  }, LIFETIME_MS[tone]);
};

export const dismiss = (id: number) => {
  items = items.filter((item) => item.id !== id);
  emit();
};

/**
 * 便捷门面：调用点遍布各页面的 catch 块，不必每次都解构。
 *
 * 用模块级 store 而不是 Context：套一层 Provider 只是为了让它们
 * 拿到同一个函数，反而增加耦合。代价是需要在根节点挂 `<ToastViewport />`。
 */
export const toast = {
  success: (title: string, description?: string) => push("success", title, description),
  error: (title: string, description?: string) => push("error", title, description),
  info: (title: string, description?: string) => push("info", title, description),
};

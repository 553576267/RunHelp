import { ref, onUnmounted } from 'vue';

export interface ActionFeedbackOptions {
  /** 冷却时长(ms)：触发后保持"进行中"状态的时长，期间重复触发被忽略。默认 600 */
  cooldownMs?: number;
  /** 防抖时长(ms)：触发后延迟多久执行动作；0 表示立即执行。默认 0 */
  debounceMs?: number;
}

/**
 * 统一的按钮交互反馈（全平台共用一处）。
 *
 * 行为与命令中心"运行"按钮一致：点击后图标原地旋转、鼠标指针保持正常，
 * 冷却期内重复点击直接忽略，冷却结束自动恢复；不等待 IPC 返回，避免卡死感。
 *
 * - 手动操作：调用 trigger(key, action)，组件用 isActive(key) 控制图标旋转。
 * - 自动触发（如定时刷新）：不要走 trigger，直接改自己的 loading 状态即可，
 *   这样自动刷新不会体现在按钮状态上，两套逻辑互不冲突。
 *
 * 参数可扩展：cooldownMs 冷却时长、debounceMs 防抖时长。
 */
export function useActionFeedback(options: ActionFeedbackOptions = {}) {
  const cooldownMs = options.cooldownMs ?? 600;
  const debounceMs = options.debounceMs ?? 0;

  const activeKeys = ref<Set<string>>(new Set());
  const cooldownTimers = new Map<string, number>();
  const debounceTimers = new Map<string, number>();

  function isActive(key: string): boolean {
    return activeKeys.value.has(key);
  }

  /** 触发一次带反馈的动作；返回 true 表示本次被接受，false 表示冷却中已忽略。 */
  function trigger(key: string, action?: () => void | Promise<void>): boolean {
    if (activeKeys.value.has(key)) return false;

    const activated = new Set(activeKeys.value);
    activated.add(key);
    activeKeys.value = activated;

    const coolTimer = window.setTimeout(() => {
      const next = new Set(activeKeys.value);
      next.delete(key);
      activeKeys.value = next;
      cooldownTimers.delete(key);
    }, cooldownMs);
    cooldownTimers.set(key, coolTimer);

    const run = () => { if (action) void action(); };
    if (debounceMs > 0) {
      const debTimer = window.setTimeout(() => {
        debounceTimers.delete(key);
        run();
      }, debounceMs);
      debounceTimers.set(key, debTimer);
    } else {
      run();
    }
    return true;
  }

  /** 清除指定 key（不传则全部）的反馈状态与定时器。 */
  function clear(key?: string): void {
    if (key === undefined) {
      cooldownTimers.forEach((t) => window.clearTimeout(t));
      debounceTimers.forEach((t) => window.clearTimeout(t));
      cooldownTimers.clear();
      debounceTimers.clear();
      activeKeys.value = new Set();
      return;
    }
    const cool = cooldownTimers.get(key);
    if (cool) window.clearTimeout(cool);
    const deb = debounceTimers.get(key);
    if (deb) window.clearTimeout(deb);
    cooldownTimers.delete(key);
    debounceTimers.delete(key);
    const next = new Set(activeKeys.value);
    next.delete(key);
    activeKeys.value = next;
  }

  onUnmounted(() => clear());

  return { activeKeys, isActive, trigger, clear };
}

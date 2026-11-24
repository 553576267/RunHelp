import { onMounted, onUnmounted, ref } from 'vue';

/**
 * 软键盘避让高度（写入 `--kb-height`）。
 *
 * 设计前提（见 MainActivity）：Android 端**不再**把 IME 高度写进 decorView 内边距，
 * 键盘是盖在页面之上的，底部页签不会跟着往上跳。需要避让的只有居中输入弹层，
 * 所以这里只负责把「键盘遮挡高度」算出来，由 CSS 决定谁上移、移多少。
 *
 * 原理：键盘弹出时 visualViewport.height 会缩小，遮挡高度 =
 *   layoutViewport 高度 - visualViewport 高度 - visualViewport.offsetTop
 * resize/scroll 在键盘动画期间会连续触发，这里用 rAF 合并，避免每帧多次写样式。
 */
export function useKeyboardAvoid() {
  const keyboardHeight = ref(0);
  let vv: VisualViewport | null = null;
  let frame = 0;

  function apply() {
    frame = 0;
    if (!vv) return;
    const h = Math.max(0, Math.round(window.innerHeight - vv.height - vv.offsetTop));
    // 抖动阈值：1px 内的变化忽略，避免键盘收起时数字来回跳
    if (Math.abs(h - keyboardHeight.value) < 1) return;
    keyboardHeight.value = h;
    document.documentElement.style.setProperty('--kb-height', `${h}px`);
  }

  function schedule() {
    if (frame) return;
    frame = window.requestAnimationFrame(apply);
  }

  onMounted(() => {
    vv = window.visualViewport ?? null;
    if (!vv) return;
    vv.addEventListener('resize', schedule);
    vv.addEventListener('scroll', schedule);
    apply();
  });

  onUnmounted(() => {
    if (frame) window.cancelAnimationFrame(frame);
    if (vv) {
      vv.removeEventListener('resize', schedule);
      vv.removeEventListener('scroll', schedule);
    }
    document.documentElement.style.setProperty('--kb-height', '0px');
  });

  /** 输入框获得焦点时的兜底：原生不滚动，只有弹层里内容超高时才需要把它滚到可见处。 */
  function focusIntoView(el: EventTarget | null) {
    const node = el as HTMLElement | null;
    if (!node) return;
    window.setTimeout(() => {
      node.scrollIntoView({ block: 'nearest', behavior: 'smooth' });
    }, 260);
  }

  return { keyboardHeight, focusIntoView };
}

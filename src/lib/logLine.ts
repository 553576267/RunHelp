import type { LogEvent, LogLineKind } from '../types';

/**
 * 日志行语义分类。
 *
 * 只在日志入队时对每行跑一次，结果挂在行对象上（`kind`），
 * 渲染与「仅错误」筛选都直接读字段，避免模板里对每一行、每一帧重复跑正则。
 *
 * 判定全部基于后端现有文案（中文 / 英文两套），不改协议。
 */

const ERROR_RE = /(?:error|exception|traceback|fatal|failed|failure|denied|refused|unhandled|cannot|invalid|not found|timeout|timed out|npm ERR|ERR!|错误|异常|失败|拒绝|无法|不存在|未找到|超时|致命)/i;

/** 警告优先于错误判定：npm 的 deprecated、git 的 LF/CRLF 都是 stderr 但不是失败 */
const WARN_RE = /(?:warn|warning|deprecated|deprecation|警告|已弃用)/i;

/** 英文版结束横幅带 Exit code，用来兜底判定成败（正常路径由 handleRunEnd 直接指定） */
const EXIT_CODE_RE = /(?:退出码|Exit code)\s*(-?\d+)/;

/** 设备状态日志里的「坏消息」：离线、探测没应答、不可达。命中的行用警告黄。 */
const DEVICE_BAD_RE = /(?:离线|未应答|不可达|失败|超时|拒绝|offline|unreachable|failed|timeout)/i;

function isBanner(s: string): boolean {
  return s.startsWith('━') && s.includes('【RunHelp】');
}

export function classifyLog(l: LogEvent): LogLineKind {
  const s = l.line;

  // 1. 横幅：开始 / 结束 / 解释器等（先判，避免被后面的错误正则误伤）
  if (isBanner(s)) {
    if (/(?:退出码|Exit code)/.test(s)) {
      if (/已停止|Stopped/i.test(s)) return 'banner-stopped';
      const m = EXIT_CODE_RE.exec(s);
      if (m) return Number(m[1]) === 0 ? 'banner-ok' : 'banner-bad';
      return /执行完成|Execution complete/i.test(s) ? 'banner-ok' : 'banner-bad';
    }
    if (/开始执行|Starting/i.test(s)) return 'banner';
    return 'meta';
  }

  // 2. 流水线步骤进度：步骤 N: 执行中 / 步骤 N 完成: 成功
  if (/^(?:步骤|Step) \d+/.test(s)) {
    if (/[:：]\s*(?:成功|Success)/i.test(s)) return 'step-ok';
    if (/[:：]\s*(?:失败|Failed)/i.test(s)) return 'step-bad';
    return 'step';
  }

  // 3. RunHelp 自己的标签行（执行命令：/ 执行结果：等）
  if (s.startsWith('【RunHelp】')) return 'meta';

  // 4. 命令回显
  if (s.startsWith('$ ')) return 'echo';

  // 5. 设备状态日志（上下线 / 探测结果）：颜色跟「设备日志」弹层保持一致——
  // 带警告意味的（离线、探测失败）黄，正常事件（上线、新发现）灰。
  // 必须排在 stderr 判定之前：这类行文案里有「失败/超时」，否则会被错误正则染红。
  if (l.stream === 'warn' || s.startsWith('[设备]')) {
    return DEVICE_BAD_RE.test(s) ? 'warn' : 'out';
  }

  // 6. 进程输出：先看 stderr 的错误/警告，其余一律按正文
  if (l.stream === 'stderr') {
    if (ERROR_RE.test(s)) {
      // "warning: failed to ..." 这类行以警告开头，按警告处理，别把整段 stderr 染红
      return /^\s*(?:warn|warning|deprecated|警告)/i.test(s) ? 'warn' : 'err';
    }
    if (WARN_RE.test(s)) return 'warn';
    return 'out';
  }
  if (l.stream === 'stdout') return 'out';

  return 'info';
}

/** 参与「错误」计数与筛选的行：错误行 + 失败横幅 + 失败步骤 */
export function isErrorLine(kind: LogLineKind): boolean {
  return kind === 'err' || kind === 'banner-bad' || kind === 'step-bad';
}

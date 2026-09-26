/** 跨页共享的轻量界面状态(T013):仅展示派生数据,不是业务状态源。
 *  pendingIssues 由概览轮询写入,侧边栏徽章与统计卡读取(US2)。
 *  chatUnread 由聊天页轮询(unread-summary)写入,侧边栏与路由徽标读取(007 T053)。 */
let pendingIssues = 0;

export function setPendingIssues(n: number): void {
  pendingIssues = n;
}
export function getPendingIssues(): number {
  return pendingIssues;
}

let chatUnread = 0;

export function setChatUnread(n: number): void {
  chatUnread = Math.max(0, Math.floor(n));
}
export function getChatUnread(): number {
  return chatUnread;
}

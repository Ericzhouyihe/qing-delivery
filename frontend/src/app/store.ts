/** 跨页共享的轻量界面状态(T013):仅展示派生数据,不是业务状态源。
 *  pendingIssues 由概览轮询写入,侧边栏徽章与统计卡读取(US2)。 */
let pendingIssues = 0;

export function setPendingIssues(n: number): void {
  pendingIssues = n;
}
export function getPendingIssues(): number {
  return pendingIssues;
}

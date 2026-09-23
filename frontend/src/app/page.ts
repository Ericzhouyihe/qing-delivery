/** 页面工厂(T013 扩展):页面可返回清理函数(停轮询/tick),
 *  路由切换前调用,防止后台残留计时器(宪章 III 资源取向)。 */
export type PageFactory = (
  root: HTMLElement
) => void | Promise<void> | (() => void) | Promise<(() => void) | void>;

/** 商品列表视图模型(007 T040):自 catalog 拆分后的纯函数(契约解析/筛选
 *  复用 catalog/products;此处仅新增深链构造)。 */

/** 「关联发货规则」跨页跳转链接:/rules 预选账号并在新建规则时预填商品。 */
export function rulesDeepLink(accountId: string, itemId: string): string {
  const sp = new URLSearchParams();
  sp.set("account", accountId);
  sp.set("item", itemId);
  return `/rules?${sp.toString()}`;
}

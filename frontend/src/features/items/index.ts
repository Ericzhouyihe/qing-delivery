export { itemsPage } from "./page";
// 自 catalog 拆分后复用导出(007 T040:商品逻辑仍以 catalog/products 为单一来源)
export { rulesDeepLink } from "./model";
export { parseItem, filterItems, parseItem as parseProductItem, type ProductItem, type AccountRef } from "../catalog/products";

export function paginate(items, page, pageSize) {
  return items.slice(page * pageSize, (page + 1) * pageSize);
}

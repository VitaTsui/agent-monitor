export interface ListRes<T> {
  list: Array<T>;
  page: {
    pageNum: number;
    pageSize: number;
    total: number;
  };
}

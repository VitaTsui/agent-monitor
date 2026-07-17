`
import React, { useEffect } from "react";

import { ChakraButtonProps, ColumnsType, FormItemProps, Panel } from "@hsu-react/ui";
import <NAME>Store from "./<NAME>Store";
import { observer } from "mobx-react-lite";
import styles from "./index.module.scss";

const <NAME>: React.FC = observer(() => {
  const {
    setSearchData,
    initSearchData,
    dataSource,
    isLoading,
    total,
    changePage,
    page,
    order,
    onOrderChange,
  } = <NAME>Store;

  useEffect(() => {
    initSearchData();
  }, [initSearchData]);

  const searchItems: FormItemProps[] = [];

  const beforeButtonGroup: ChakraButtonProps[] = [];

  const columns: ColumnsType = [];

  return (
    <>
      <Panel.List
        className={styles.<NAME>}
        searchProps={{
          searchItems,
          onSearch: setSearchData,
          onReset: initSearchData,
          beforeButtonGroup,
          hasPermi: [<QUERY_PERMI>],
        }}
        tableProps={{
          columns,
          dataSource,
          rowKey: "id",
          loading: isLoading,
          pagination: {
            total,
            onChange: (num, size) => changePage({ num, size }),
            current: page?.num,
            pageSize: page?.size,
          },
          order,
          onOrderChange,
        }}
      />
    </>
  );
});

export default <NAME>;
`;

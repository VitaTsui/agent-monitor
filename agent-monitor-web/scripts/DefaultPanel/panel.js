`
import React from "react";

import { Panel } from "@hsu-react/ui";
import { observer } from "mobx-react-lite";
import styles from "./index.module.scss";

const <NAME>: React.FC = observer(() => {
  return (
    <>
      <Panel.Default
        className={styles.<NAME>}
        contentClassName={styles.<NAME>Content}
      >
      </Panel.Default>
    </>
  );
});

export default <NAME>;
`;

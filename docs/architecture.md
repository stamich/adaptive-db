# Milestone 1.6 architecture

```text
                         Rust data plane

             +------------------------------+
             | Transaction / MVCC           |
             +---------------+--------------+
                             |
                             v
             +------------------------------+
             | Segmented logical WAL        |
             | BEGIN / VERSION / PUT /      |
             | DELETE / COMMIT              |
             +---------------+--------------+
                             |
                            fsync
                             |
              +--------------+--------------+
              |                             |
              v                             v
    +--------------------+       +----------------------+
    | Persistent Current |       | Persistent Versions  |
    |                    |       |                      |
    | RowId              |       | (RowId, BeginTs)     |
    |   -> B+Tree        |       |    -> VersionBTree   |
    |   -> RowLocation   |       |    -> RowLocation    |
    |   -> Heap page     |       |    -> Version heap   |
    +----------+---------+       +-----------+----------+
               |                             |
               +-------------+---------------+
                             |
                             v
                    +----------------+
                    | Checkpoint v2  |
                    +----------------+
```

Before-images są zapisane w WAL jawnie. Dzięki temu crash po utrwaleniu
Current Store, ale przed utrwaleniem Version Store nie powoduje utraty
historycznej wartości.

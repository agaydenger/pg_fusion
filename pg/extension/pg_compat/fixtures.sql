SET extra_float_digits = 0;

CREATE TEMP TABLE char_tbl(f1 char(4));
INSERT INTO char_tbl VALUES ('a'), ('ab'), ('abcd'), ('abcd    ');
ANALYZE char_tbl;

CREATE TEMP TABLE text_tbl(f1 text);
INSERT INTO text_tbl VALUES ('doh!'), ('hi de ho neighbor');
ANALYZE text_tbl;

CREATE TEMP TABLE varchar_tbl(f1 varchar(4));
INSERT INTO varchar_tbl VALUES ('a'), ('ab'), ('abcd'), ('abcd    ');
ANALYZE varchar_tbl;

CREATE TEMP TABLE booltbl1(f1 bool);
INSERT INTO booltbl1 VALUES (true), (true), (true), (false);
ANALYZE booltbl1;

CREATE TEMP TABLE booltbl2(f1 bool);
INSERT INTO booltbl2 VALUES (false), (false), (false), (false);
ANALYZE booltbl2;

CREATE TEMP TABLE onek (
    unique1 int4,
    unique2 int4,
    two int4,
    four int4,
    ten int4,
    twenty int4,
    hundred int4,
    thousand int4,
    twothousand int4,
    fivethous int4,
    tenthous int4,
    odd int4,
    even int4,
    stringu1 text,
    stringu2 text,
    string4 text
);

INSERT INTO onek
SELECT
    g,
    999 - g,
    g % 2,
    g % 4,
    g % 10,
    g % 20,
    g % 100,
    g % 1000,
    g % 2000,
    g % 5000,
    g % 10000,
    CASE WHEN g % 2 = 1 THEN g ELSE NULL END,
    CASE WHEN g % 2 = 0 THEN g ELSE NULL END,
    'U' || lpad(g::text, 4, '0'),
    'V' || lpad((999 - g)::text, 4, '0'),
    'S' || (g % 4)::text
FROM generate_series(0, 999) AS g;

ANALYZE onek;

CREATE TEMP TABLE onek2 AS SELECT * FROM onek;
ANALYZE onek2;

CREATE TEMP TABLE tenk1 AS
SELECT
    g AS unique1,
    1999 - g AS unique2,
    g % 2 AS two,
    g % 4 AS four,
    g % 10 AS ten,
    g % 20 AS twenty,
    g % 100 AS hundred,
    g % 1000 AS thousand,
    g % 2000 AS twothousand,
    g % 5000 AS fivethous,
    g % 10000 AS tenthous,
    CASE WHEN g % 2 = 1 THEN g ELSE NULL END AS odd,
    CASE WHEN g % 2 = 0 THEN g ELSE NULL END AS even,
    'TU' || lpad(g::text, 4, '0') AS stringu1,
    'TV' || lpad((1999 - g)::text, 4, '0') AS stringu2,
    'TS' || (g % 4)::text AS string4
FROM generate_series(0, 1999) AS g;

ANALYZE tenk1;

CREATE TEMP TABLE tenk2 AS SELECT * FROM tenk1;
ANALYZE tenk2;

CREATE TEMP TABLE int2_tbl(f1 int2);
INSERT INTO int2_tbl VALUES (0), (1234), (-1234), (32767), (-32767);
ANALYZE int2_tbl;

CREATE TEMP TABLE int4_tbl(f1 int4);
INSERT INTO int4_tbl VALUES (0), (123456), (-123456), (2147483647), (-2147483647);
ANALYZE int4_tbl;

CREATE TEMP TABLE int8_tbl(q1 int8, q2 int8);
INSERT INTO int8_tbl VALUES
    (123, 456),
    (123, 4567890123456789),
    (4567890123456789, 123),
    (4567890123456789, 4567890123456789),
    (4567890123456789, -4567890123456789);
ANALYZE int8_tbl;

CREATE TEMP TABLE float4_tbl(f1 float4);
INSERT INTO float4_tbl VALUES (0.0), (1004.30), (-34.84), (1.2345678e20), (1.2345678e-20);
ANALYZE float4_tbl;

CREATE TEMP TABLE float8_tbl(f1 float8);
INSERT INTO float8_tbl VALUES (0.0), (1004.30), (-34.84), (1.2345678901234e200), (1.2345678901234e-200);
ANALYZE float8_tbl;

CREATE TEMP TABLE aggtest(a int2, b float4);
INSERT INTO aggtest VALUES (10, 1.5), (20, 2.5), (30, 3.5), (100, 4.5), (120, 5.5);
ANALYZE aggtest;

CREATE TEMP TABLE case_tbl(i int4, f float8);
INSERT INTO case_tbl VALUES (1, 1.0), (2, 2.0), (3, NULL), (4, 4.0), (101, 101.5);
ANALYZE case_tbl;

CREATE TEMP TABLE case2_tbl(i int4, j int4);
INSERT INTO case2_tbl VALUES (1, -1), (2, -2), (3, -3), (2, -4), (1, NULL), (NULL, -6);
ANALYZE case2_tbl;

CREATE TEMP TABLE foo(f1 int4);
INSERT INTO foo VALUES (42), (3), (10), (7), (NULL), (NULL), (1);
ANALYZE foo;

CREATE TEMP TABLE j1_tbl(i int4, j int4, t text);
CREATE TEMP TABLE j2_tbl(i int4, k int4);

INSERT INTO j1_tbl VALUES
    (1, 4, 'one'),
    (2, 3, 'two'),
    (3, 2, 'three'),
    (4, 1, 'four'),
    (5, 0, 'five'),
    (6, 6, 'six'),
    (7, 7, 'seven'),
    (8, 8, 'eight'),
    (0, NULL, 'zero'),
    (NULL, NULL, 'null'),
    (NULL, 0, 'zero');

INSERT INTO j2_tbl VALUES
    (1, -1),
    (2, 2),
    (3, -3),
    (2, 4),
    (5, -5),
    (5, -5),
    (0, NULL),
    (NULL, NULL),
    (NULL, 0);

ANALYZE j1_tbl;
ANALYZE j2_tbl;

-- IC table (#59): per feature, per year, per horizon — the daily Spearman rank
-- correlation between the feature's cross-section and forward excess return,
-- averaged over the year. `ir` = mean / stddev of the daily ICs.
-- Needs a wide enough cross-section: HAVING count() >= 30 names per day.
SELECT feature, toYear(day) AS year, horizon,
       round(avg(ic), 4)                   AS mean_ic,
       round(avg(ic) / stddevSamp(ic), 2)  AS ir,
       count()                              AS days
FROM (
    SELECT f.feature AS feature, f.day AS day, o.horizon AS horizon,
           rankCorr(f.value, o.excess) AS ic
    FROM alpaca.features f
    JOIN alpaca.outcomes o ON o.symbol = f.symbol AND o.day = f.day
    GROUP BY feature, day, horizon
    HAVING count() >= 30
)
GROUP BY feature, year, horizon
ORDER BY feature, year, horizon;

#!/usr/bin/env python3
"""
yfinance proxy for tagent (Rust)
Wraps Python yfinance library so Rust can call it via subprocess.
Returns JSON for easy parsing.
"""

import sys
import json
import os
from datetime import datetime

# Ensure utf-8 output
if sys.platform == "win32":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.stderr.reconfigure(encoding="utf-8", errors="replace")

import yfinance as yf


def get_stock_data(symbol: str, start_date: str, end_date: str) -> dict:
    """Fetch OHLCV stock data for a date range."""
    try:
        ticker = yf.Ticker(symbol.upper())
        data = ticker.history(start=start_date, end=end_date)

        if data.empty:
            return {"error": f"No data found for {symbol} between {start_date} and {end_date}"}

        # Remove timezone
        if data.index.tz is not None:
            data.index = data.index.tz_localize(None)

        records = []
        for ts, row in data.iterrows():
            records.append({
                "date": ts.strftime("%Y-%m-%d"),
                "open": round(float(row["Open"]), 2),
                "high": round(float(row["High"]), 2),
                "low": round(float(row["Low"]), 2),
                "close": round(float(row["Close"]), 2),
                "volume": int(row["Volume"]),
            })

        return {
            "symbol": symbol.upper(),
            "start_date": start_date,
            "end_date": end_date,
            "records": records,
            "count": len(records),
        }
    except Exception as e:
        return {"error": str(e)}


def get_indicators(symbol: str, curr_date: str, look_back_days: int = 30) -> dict:
    """Fetch technical indicators (RSI, MACD, Bollinger)."""
    try:
        from dateutil.relativedelta import relativedelta

        end = datetime.strptime(curr_date, "%Y-%m-%d")
        start = end - relativedelta(days=look_back_days)

        ticker = yf.Ticker(symbol.upper())
        data = ticker.history(start=start.strftime("%Y-%m-%d"), end=curr_date)

        if data.empty:
            return {"error": f"No data found for {symbol}"}

        closes = data["Close"].tolist()

        # RSI (14)
        rsi = None
        if len(closes) >= 15:
            gains, losses = [], []
            for i in range(1, len(closes)):
                diff = closes[i] - closes[i - 1]
                gains.append(diff if diff > 0 else 0.0)
                losses.append(abs(diff) if diff < 0 else 0.0)
            avg_gain = sum(gains[-14:]) / 14
            avg_loss = sum(losses[-14:]) / 14
            if avg_loss == 0:
                rsi = 100.0
            else:
                rs = avg_gain / avg_loss
                rsi = round(100.0 - (100.0 / (1.0 + rs)), 2)

        # Bollinger Bands (20, 2)
        bb_upper, bb_middle, bb_lower = None, None, None
        if len(closes) >= 20:
            window = closes[-20:]
            mean = sum(window) / 20
            variance = sum((x - mean) ** 2 for x in window) / 20
            std = variance ** 0.5
            bb_middle = round(mean, 2)
            bb_upper = round(mean + 2 * std, 2)
            bb_lower = round(mean - 2 * std, 2)

        # Simple MA
        sma10 = round(sum(closes[-10:]) / 10, 2) if len(closes) >= 10 else None
        sma20 = round(sum(closes[-20:]) / 20, 2) if len(closes) >= 20 else None

        return {
            "symbol": symbol.upper(),
            "curr_date": curr_date,
            "look_back_days": look_back_days,
            "rsi_14": rsi,
            "bb_upper": bb_upper,
            "bb_middle": bb_middle,
            "bb_lower": bb_lower,
            "sma_10": sma10,
            "sma_20": sma20,
            "current_price": round(closes[-1], 2) if closes else None,
        }
    except Exception as e:
        return {"error": str(e)}


def get_financials(ticker: str) -> dict:
    """Fetch company fundamentals."""
    try:
        ticker_obj = yf.Ticker(ticker.upper())
        info = ticker_obj.info

        return {
            "symbol": ticker.upper(),
            "company_name": info.get("longName") or info.get("shortName", ""),
            "sector": info.get("sector", ""),
            "industry": info.get("industry", ""),
            "market_cap": info.get("marketCap"),
            "pe_ratio": info.get("trailingPE"),
            "eps": info.get("trailingEps"),
            "dividend_yield": info.get("dividendYield"),
            "52w_high": info.get("fiftyTwoWeekHigh"),
            "52w_low": info.get("fiftyTwoWeekLow"),
            "price": info.get("currentPrice") or info.get("regularMarketPrice"),
        }
    except Exception as e:
        return {"error": str(e)}


def _parse_news_item(item: dict) -> dict:
    """Parse a news item from yfinance, handling nested content structure."""
    # yfinance news items have fields either at top level or nested in "content"
    content = item.get("content", item)

    title = content.get("title", "")
    description = content.get("description", "")
    summary = content.get("summary", "")
    provider = content.get("provider", {})
    if isinstance(provider, dict):
        source = provider.get("displayName", "")
    else:
        source = str(provider) if provider else ""

    canonical = content.get("canonicalUrl", {})
    if isinstance(canonical, dict):
        link = canonical.get("url", "")
    else:
        link = str(canonical) if canonical else ""

    clickthrough = content.get("clickThroughUrl", {})
    if isinstance(clickthrough, dict):
        click_url = clickthrough.get("url", "")
        if not link:
            link = click_url
    else:
        click_url = str(clickthrough) if clickthrough else ""

    pub_date = content.get("pubDate", "") or content.get("displayTime", "")

    # Use description/summary as fallback for title
    if not title and description:
        title = description[:200]
    elif not title and summary:
        title = summary[:200]

    return {
        "title": title or "(无标题)",
        "source": source or "未知来源",
        "link": link or "",
        "pub_date": pub_date or "",
    }


def get_news(ticker: str, start_date: str, end_date: str) -> dict:
    """Fetch recent news for a ticker. Tries multiple yfinance methods."""
    try:
        ticker_obj = yf.Ticker(ticker.upper())

        # Try get_news() first
        news = ticker_obj.get_news()

        articles = []
        has_content = False

        if news and len(news) > 0:
            for item in news[:10]:
                parsed = _parse_news_item(item)
                if parsed["title"] != "(无标题)" or parsed["source"] != "未知来源":
                    has_content = True
                articles.append(parsed)

        # Fallback: try ticker.news attribute if get_news() returned empty or content-free results
        if not has_content:
            try:
                news_attr = getattr(ticker_obj, "news", None)
                if news_attr and len(news_attr) > 0:
                    articles = []
                    for item in news_attr[:10]:
                        parsed = _parse_news_item(item)
                        if parsed["title"] != "(无标题)" or parsed["source"] != "未知来源":
                            has_content = True
                        articles.append(parsed)
            except Exception:
                pass  # Fallback failed, keep existing articles

        # If still no content, return explicit "no news found" marker
        if not has_content:
            return {
                "symbol": ticker.upper(),
                "articles": [],
                "count": 0,
                "status": "no_news_coverage",
                "note": f"No recent news coverage found for {ticker.upper()}. This may indicate low analyst/media attention for this security.",
            }

        return {
            "symbol": ticker.upper(),
            "articles": articles,
            "count": len(articles),
        }
    except Exception as e:
        return {"error": str(e)}


if __name__ == "__main__":
    if len(sys.argv) < 3:
        print(json.dumps({"error": "Usage: python yfinance_proxy.py <action> <args...>"}))
        sys.exit(1)

    action = sys.argv[1]

    result = None
    if action == "get_stock_data":
        symbol, start_date, end_date = sys.argv[2], sys.argv[3], sys.argv[4]
        result = get_stock_data(symbol, start_date, end_date)
    elif action == "get_indicators":
        symbol, curr_date, look_back = sys.argv[2], sys.argv[3], int(sys.argv[4]) if len(sys.argv) > 4 else 30
        result = get_indicators(symbol, curr_date, look_back)
    elif action == "get_financials":
        ticker = sys.argv[2]
        result = get_financials(ticker)
    elif action == "get_news":
        ticker, start_date, end_date = sys.argv[2], sys.argv[3], sys.argv[4]
        result = get_news(ticker, start_date, end_date)
    else:
        result = {"error": f"Unknown action: {action}"}

    print(json.dumps(result, ensure_ascii=False))

from pathlib import Path


def main():
    directory = Path(__file__).resolve().parent.parent / "docs" / "explainer"
    source = (directory / "index.html").read_text()
    prefix = (
        '<!doctype html>\n<html lang="zh-CN">\n<head>\n'
        '<meta charset="utf-8">\n'
        '<meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">\n'
        '<link rel="icon" href="data:,">\n'
        '</head>\n<body>\n'
    )
    (directory / "cordis-explainer.html").write_text(prefix + source + '\n</body>\n</html>\n')


if __name__ == "__main__":
    main()

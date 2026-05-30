import argparse
from flask import Flask

app = Flask(__name__)

parser = argparse.ArgumentParser()
parser.add_argument("--mode", default="default")
parsed = parser.parse_args()


@app.route("/")
def index():
    return parsed.mode, 200


if __name__ == "__main__":
    app.run(host="0.0.0.0", port=8080)

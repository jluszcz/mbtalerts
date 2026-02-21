terraform {
  backend "s3" {
    bucket = "jluszcz-tf-state"
    key    = "mbtalerts"
    region = "us-east-2"
  }
}

# Sourced from environment variables named TF_VAR_${VAR_NAME}
variable "aws_region" {}

variable "code_bucket" {}

variable calendar_id {}

variable service_acct_key {}

provider "aws" {
  region = var.aws_region
}

data "aws_s3_bucket" "code_bucket" {
  bucket = var.code_bucket
}

// Run daily at 15:00 UTC
resource "aws_cloudwatch_event_rule" "schedule" {
  name                = "mbtalerts-schedule"
  schedule_expression = "cron(0 15 * * ? *)"
}

resource "aws_cloudwatch_event_target" "schedule_target" {
  rule = aws_cloudwatch_event_rule.schedule.name
  arn  = aws_lambda_function.mbtalerts.arn
}

resource "aws_lambda_permission" "cw_execution" {
  statement_id  = "mbtalerts-AllowExecutionFromCloudWatch"
  action        = "lambda:InvokeFunction"
  function_name = aws_lambda_function.mbtalerts.arn
  principal     = "events.amazonaws.com"
  source_arn    = aws_cloudwatch_event_rule.schedule.arn
}

data "aws_iam_policy_document" "assume_role" {
  statement {
    principals {
      type        = "Service"
      identifiers = ["lambda.amazonaws.com"]
    }
    actions = ["sts:AssumeRole"]
  }
}

resource "aws_iam_role" "role" {
  name               = "mbtalerts.lambda"
  assume_role_policy = data.aws_iam_policy_document.assume_role.json
}

data "aws_iam_policy_document" "cw" {
  statement {
    actions   = ["cloudwatch:PutMetricData"]
    resources = ["*"]
    condition {
      test     = "StringEquals"
      variable = "cloudwatch:namespace"
      values   = ["mbtalerts"]
    }
  }
}

resource "aws_iam_policy" "cw" {
  name   = "mbtalerts.cw"
  policy = data.aws_iam_policy_document.cw.json
}

resource "aws_iam_role_policy_attachment" "cw" {
  role       = aws_iam_role.role.name
  policy_arn = aws_iam_policy.cw.arn
}

resource "aws_iam_role_policy_attachment" "basic_execution_role_attachment" {
  role       = aws_iam_role.role.name
  policy_arn = "arn:aws:iam::aws:policy/service-role/AWSLambdaBasicExecutionRole"
}

resource "aws_lambda_function" "mbtalerts" {
  function_name = "mbtalerts"
  s3_bucket     = data.aws_s3_bucket.code_bucket.bucket
  s3_key        = "mbtalerts.zip"
  role          = aws_iam_role.role.arn
  architectures = ["arm64"]
  runtime       = "provided.al2023"
  handler       = "ignored"
  publish       = "false"
  description   = "Sync MBTA Alerts against Google Calendars"
  timeout       = 30
  memory_size   = 128

  environment {
    variables = {
      GOOGLE_CALENDAR_ID          = var.calendar_id
      GOOGLE_SERVICE_ACCOUNT_KEY  = var.service_acct_key
    }
  }
}

resource "aws_cloudwatch_log_group" "log_group" {
  name              = "/aws/lambda/mbtalerts"
  retention_in_days = "7"
}

data "aws_iam_openid_connect_provider" "github" {
  url = "https://token.actions.githubusercontent.com"
}

data "aws_iam_policy_document" "github" {
  statement {
    actions   = ["s3:PutObject"]
    resources = ["${data.aws_s3_bucket.code_bucket.arn}/mbtalerts.zip"]
  }
}

resource "aws_iam_policy" "github" {
  name   = "mbtalerts.github"
  policy = data.aws_iam_policy_document.github.json
}

resource "aws_iam_role" "github" {
  name = "mbtalerts.github"

  assume_role_policy = jsonencode({
    Version = "2012-10-17",
    Statement = [
      {
        Effect = "Allow",
        Principal = {
          Federated = data.aws_iam_openid_connect_provider.github.arn
        },
        Action = "sts:AssumeRoleWithWebIdentity",
        Condition = {
          StringEquals = {
            "token.actions.githubusercontent.com:aud" : "sts.amazonaws.com"
          }
          StringLike = {
            "token.actions.githubusercontent.com:sub" : "repo:jluszcz/mbtalerts:*"
          },
        }
      }
    ]
  })
}

resource "aws_iam_role_policy_attachment" "github" {
  role       = aws_iam_role.github.name
  policy_arn = aws_iam_policy.github.arn
}

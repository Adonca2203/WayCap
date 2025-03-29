#!/bin/bash

# Helper script which uses ffmpeg to clip files using ffmpeg
#
# Given an input file name -i, start time -s, end time -e, and output file name -o
# create a copy containing only the bits between the timestamps inclusive
#
# EXAMPLE USAGE:
# ./copy.sh -i input.mp4 -s 00:04:30 -e 00:04:50 -o output.mp4

INPUT_FILE=""
START_TIME=""
END_TIME=""
OUTPUT_NAME=""

# Parse command-line arguments
while getopts ":i:s:e:o:" opt; do
  case ${opt} in
    i)
      INPUT_FILE=$OPTARG
      ;;
    s)
      START_TIME=$OPTARG
      ;;
    e)
      END_TIME=$OPTARG
      ;;
    o)
      OUTPUT_NAME=$OPTARG
      ;;
    \?)
      echo "Usage: $0 -i input_file -s start_time -e end_time -o output_name"
      exit 1
      ;;
  esac
done

if [[ -z "$INPUT_FILE" || -z "$START_TIME" || -z "$END_TIME" || -z "$OUTPUT_NAME" ]]; then
  echo "Missing required arguments."
  echo "Usage: $0 -i input_file -s start_time -e end_time -o output_name"
  exit 1
fi

ffmpeg -i "$INPUT_FILE" -ss "$START_TIME" -to "$END_TIME" -c copy "$OUTPUT_NAME"

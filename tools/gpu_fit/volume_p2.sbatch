#!/bin/bash
#SBATCH --job-name=voxel-P2-1km
#SBATCH --account=pi_sk2433
#SBATCH --partition=gpu
#SBATCH --gpus=1
#SBATCH --cpus-per-task=8
#SBATCH --mem=32G
#SBATCH --time=02:00:00
#SBATCH --signal=B:USR1@180
#SBATCH --output=logs/volume-p2-%j.out
#SBATCH --error=logs/volume-p2-%j.err
source gpu_fit/volume_run.sh p2
